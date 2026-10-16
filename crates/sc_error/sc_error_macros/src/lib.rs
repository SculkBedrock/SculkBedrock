//! `#[derive(ScError)]` for domain errors.
//!
//! Each enum variant (or the struct itself) carries `detail: String` and
//! `site: ::sc_error::ScSite` fields plus a `#[sc_error(code = "...")]`
//! attribute naming its stable code:
//!
//! ```ignore
//! #[derive(Debug, ScError)]
//! pub enum WorldStorageError {
//!     #[sc_error(code = "world.storage.io")]
//!     Io { source: std::io::Error, detail: String, site: ScSite },
//! }
//! ```

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Lit, Meta, WherePredicate};

/// Derive `::sc_error::ScError` from per-variant `#[sc_error(code = "...")]`.
///
/// Generic errors add a container bound:
///
/// ```ignore
/// #[derive(Debug, ScError)]
/// #[sc_error(bound = "E: ScError + 'static")]
/// pub enum NetQueueError<E> { /* ... */ }
/// ```
#[proc_macro_derive(ScError, attributes(sc_error))]
pub fn derive_sc_error(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match implement(&ast) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn implement(ast: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let extra_bound: Option<WherePredicate> = container_bound(&ast.attrs)?;
    let mut predicates: Vec<WherePredicate> = where_clause
        .map(|clause| clause.predicates.iter().cloned().collect())
        .unwrap_or_default();
    if let Some(bound) = extra_bound {
        predicates.push(bound);
    }
    let where_clause_with_extra = if predicates.is_empty() {
        quote! {}
    } else {
        quote! { where #(#predicates),* }
    };

    match &ast.data {
        Data::Enum(data_enum) => {
            let mut code_arms = Vec::new();
            let mut site_arms = Vec::new();
            let mut detail_arms = Vec::new();
            for variant in &data_enum.variants {
                let variant_name = &variant.ident;
                let code = variant_code(&variant.attrs, &variant_name.to_string())?;
                match &variant.fields {
                    Fields::Named(_) => {
                        code_arms.push(quote! {
                            Self::#variant_name { .. } => ::sc_error::ScCode::new(#code),
                        });
                        site_arms.push(quote! {
                            Self::#variant_name { site, .. } => Some(*site),
                        });
                        detail_arms.push(quote! {
                            Self::#variant_name { detail, .. } => detail,
                        });
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            variant,
                            "ScError variants must be struct variants with `detail` and `site` fields",
                        ));
                    }
                }
            }
            Ok(quote! {
                impl #impl_generics ::sc_error::ScError for #name #ty_generics
                #where_clause_with_extra
                {
                    fn code(&self) -> ::sc_error::ScCode {
                        match self {
                            #(#code_arms)*
                        }
                    }

                    fn site(&self) -> Option<::sc_error::ScSite> {
                        match self {
                            #(#site_arms)*
                        }
                    }

                    fn detail(&self) -> &str {
                        match self {
                            #(#detail_arms)*
                        }
                    }
                }
            })
        }
        Data::Struct(data_struct) => {
            match &data_struct.fields {
                Fields::Named(_) => {}
                _ => {
                    return Err(syn::Error::new_spanned(
                        &ast.ident,
                        "ScError structs must have `detail` and `site` fields",
                    ));
                }
            }
            let code = variant_code(&ast.attrs, &name.to_string())?;
            Ok(quote! {
                impl #impl_generics ::sc_error::ScError for #name #ty_generics
                #where_clause_with_extra
                {
                    fn code(&self) -> ::sc_error::ScCode {
                        ::sc_error::ScCode::new(#code)
                    }

                    fn site(&self) -> Option<::sc_error::ScSite> {
                        Some(self.site)
                    }

                    fn detail(&self) -> &str {
                        &self.detail
                    }
                }
            })
        }
        _ => Err(syn::Error::new_spanned(
            &ast.ident,
            "ScError can only be derived for enums and structs",
        )),
    }
}

/// Read `bound = "..."` from `#[sc_error(bound = "...")]`.
fn container_bound(attrs: &[syn::Attribute]) -> syn::Result<Option<WherePredicate>> {
    for attr in attrs {
        if !attr.path().is_ident("sc_error") {
            continue;
        }
        if let Meta::List(list) = &attr.meta {
            let parsed: Meta = list
                .parse_args()
                .map_err(|_| syn::Error::new_spanned(attr, "expected #[sc_error(bound = \"...\")]"))?;
            if let Meta::NameValue(pair) = parsed {
                if pair.path.is_ident("bound") {
                    if let syn::Expr::Lit(expr) = &pair.value {
                        if let Lit::Str(text) = &expr.lit {
                            let bound: WherePredicate =
                                syn::parse_str(&text.value()).map_err(|_| {
                                    syn::Error::new_spanned(attr, "invalid bound predicate")
                                })?;
                            return Ok(Some(bound));
                        }
                    }
                }
            }
        }
    }
    Ok(None)
}

/// Read `code = "..."` from `#[sc_error(code = "...")]`.
fn variant_code(attrs: &[syn::Attribute], what: &str) -> syn::Result<String> {
    for attr in attrs {
        if !attr.path().is_ident("sc_error") {
            continue;
        }
        if let Meta::List(list) = &attr.meta {
            let parsed: Meta = list.parse_args().map_err(|_| {
                syn::Error::new_spanned(attr, "expected #[sc_error(code = \"...\")]")
            })?;
            if let Meta::NameValue(pair) = parsed {
                if pair.path.is_ident("code") {
                    if let syn::Expr::Lit(expr) = &pair.value {
                        if let Lit::Str(text) = &expr.lit {
                            return Ok(text.value());
                        }
                    }
                }
            }
        }
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        format!("missing #[sc_error(code = \"...\")] on {what}"),
    ))
}
