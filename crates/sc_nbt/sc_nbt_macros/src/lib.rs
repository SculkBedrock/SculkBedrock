use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use proc_macro_crate::FoundCrate;
use quote::quote;
use syn::{parse_macro_input, Data, DataStruct, DeriveInput, Error, Path};

fn get_path(crate_name: &str, span: Span) -> Path {
    let found_crate = proc_macro_crate::crate_name(crate_name).unwrap();

    match found_crate {
        FoundCrate::Itself => Path::from(Ident::new("crate", span)),
        FoundCrate::Name(name) => Path::from(Ident::new(&name, span)),
    }
}

#[proc_macro_derive(NbtWrite)]
pub fn nbt_write_drive(input: TokenStream) -> TokenStream {
    let input = input.clone();
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;

    let path = get_path("sc_nbt", name.span());
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();
    let fields = match input.data {
        Data::Struct(DataStruct { ref fields, .. }) => fields.iter(),
        _ => {
            return TokenStream::from(
                Error::new_spanned(name, "NbtIo derive only support struct").to_compile_error(),
            );
        }
    };
    // Collect each field's ident and type.
    let field_ident = fields
        .clone()
        .map(|field| {
            let field_type = field.ident.as_ref().unwrap();
            field_type.clone()
        })
        .collect::<Vec<_>>();
    let field_ty = fields
        .map(|field| {
            let field_type = &field.ty;
            field_type.clone()
        })
        .collect::<Vec<_>>();

    TokenStream::from(quote! {
        impl #impl_generics #path::writer::NbtCustomWrite for #name #type_generics #where_clause {
            fn write<T: #path::writer::NbtWriteTrait>(&self, writer: &mut #path::writer::NbtWriter) -> std::io::Result<()> {
                writer.write::<T>(&self.to_nbt().unwrap())
            }

            fn to_nbt(&self) -> Option<#path::NbtValue> {
                let mut compound = #path::compound::CompoundNbt::new(None);
                #(
                    if let Some(value) = <#field_ty as #path::writer::NbtCustomWrite>::to_nbt(&self.#field_ident) {
                        compound.insert(stringify!(#field_ident), value);
                    }
                )*
                Some(#path::NbtValue::Compound(compound))
            }
        }
    })
}

#[proc_macro_derive(NbtFacet, attributes(nbt))]
pub fn nbt_facet_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let path = get_path("sc_nbt", name.span());

    // Check struct-level #[nbt(tag = "...")]
    let mut tag_name: Option<String> = None;
    for attr in &input.attrs {
        if attr.path().is_ident("nbt") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("tag") {
                    let value = meta.value()?;
                    let s: syn::LitStr = value.parse()?;
                    tag_name = Some(s.value());
                    Ok(())
                } else {
                    Ok(())
                }
            });
        }
    }

    let fields = match input.data {
        Data::Struct(DataStruct { ref fields, .. }) => fields,
        _ => {
            return TokenStream::from(
                Error::new_spanned(name, "NbtFacet only supports struct").to_compile_error(),
            );
        }
    };

    let mut read_fields = Vec::new();
    let mut write_fields = Vec::new();
    let mut remove_fields = Vec::new();

    for field in fields {
        let field_ident = field.ident.as_ref().unwrap();
        let field_ty = &field.ty;

        let mut key_name = field_ident.to_string();
        for attr in &field.attrs {
            if attr.path().is_ident("nbt") {
                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("rename") {
                        let value = meta.value()?;
                        let s: syn::LitStr = value.parse()?;
                        key_name = s.value();
                        Ok(())
                    } else {
                        Ok(())
                    }
                });
            }
        }

        read_fields.push(quote! {
            #field_ident: <#field_ty as #path::NbtField>::from_field(target.get(#key_name))?,
        });

        write_fields.push(quote! {
            <#field_ty as #path::NbtField>::write_field(&self.#field_ident, #key_name, target);
        });

        remove_fields.push(quote! {
            target.remove(#key_name);
        });
    }

    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let expanded = match tag_name {
        Some(tag) => quote! {
            impl #impl_generics #path::NbtFacet for #name #type_generics #where_clause {
                fn read_from(root: &#path::CompoundNbt) -> Option<Self> {
                    let target = root.get(#tag)?.as_compound()?;
                    Some(Self {
                        #(#read_fields)*
                    })
                }

                fn write_to(&self, root: &mut #path::CompoundNbt) {
                    let mut sub = root
                        .get(#tag)
                        .and_then(|v| v.as_compound())
                        .cloned()
                        .unwrap_or_else(|| #path::CompoundNbt::new(None));
                    let target = &mut sub;
                    #(#write_fields)*
                    root.insert(#tag, #path::NbtValue::Compound(sub));
                }

                fn remove_from(root: &mut #path::CompoundNbt) {
                    root.remove(#tag);
                }
            }
        },
        None => quote! {
            impl #impl_generics #path::NbtFacet for #name #type_generics #where_clause {
                fn read_from(root: &#path::CompoundNbt) -> Option<Self> {
                    let target = root;
                    Some(Self {
                        #(#read_fields)*
                    })
                }

                fn write_to(&self, root: &mut #path::CompoundNbt) {
                    let target = root;
                    #(#write_fields)*
                }

                fn remove_from(root: &mut #path::CompoundNbt) {
                    let target = root;
                    #(#remove_fields)*
                }
            }
        },
    };

    TokenStream::from(expanded)
}
