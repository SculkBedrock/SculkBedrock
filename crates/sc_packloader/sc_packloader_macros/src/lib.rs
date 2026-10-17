//! Proc macros for `sc_packloader`.
//!
//! - [`BlockComponentSchema`]: derives the block-component registry impl from
//!   a `#[block_component("<key>")]` attribute, replacing hand-written
//!   `impl BlockComponentSchema for X { const NAME: ... }` blocks.

use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use proc_macro_crate::FoundCrate;
use quote::quote;
use syn::parse_macro_input;
use syn::spanned::Spanned;

fn get_path(crate_name: &str, span: Span) -> syn::Path {
    let found_crate = proc_macro_crate::crate_name(crate_name).unwrap();
    match found_crate {
        FoundCrate::Itself => syn::Path::from(Ident::new("crate", span)),
        FoundCrate::Name(name) => syn::Path::from(Ident::new(&name, span)),
    }
}

/// Derive the [`sc_packloader::block::component::BlockComponentSchema`] impl
/// from a `#[block_component("<key>")]` attribute:
///
/// ```ignore
/// #[derive(Clone, Component, Deserialize, Debug, BlockComponentSchema)]
/// #[block_component("minecraft:collision_box")]
/// pub struct CollisionBox { ... }
/// ```
#[proc_macro_derive(BlockComponentSchema, attributes(block_component))]
pub fn block_component_schema_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let span = input.span();
    let name = input.ident.clone();
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let mut key: Option<syn::LitStr> = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("block_component") {
            continue;
        }
        match attr.parse_args::<syn::LitStr>() {
            Ok(literal) => {
                if key.is_some() {
                    return syn::Error::new_spanned(attr, "duplicate `block_component` attribute")
                        .to_compile_error()
                        .into();
                }
                key = Some(literal);
            }
            Err(error) => {
                return syn::Error::new_spanned(
                    attr,
                    format!("`block_component` expects a string key: {error}"),
                )
                .to_compile_error()
                .into();
            }
        }
    }
    let Some(key) = key else {
        return syn::Error::new_spanned(
            &input.ident,
            "missing `#[block_component(\"<key>\")]` attribute",
        )
        .to_compile_error()
        .into();
    };

    let packloader_path = get_path("sc_packloader", span);
    TokenStream::from(quote! {
        impl #impl_generics #packloader_path::block::component::BlockComponentSchema
            for #name #type_generics #where_clause
        {
            const NAME: &'static str = #key;
        }
    })
}
