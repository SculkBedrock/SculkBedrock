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
