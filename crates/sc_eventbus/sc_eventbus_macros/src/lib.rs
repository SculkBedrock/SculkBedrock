use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use proc_macro_crate::FoundCrate;
use quote::quote;
use syn::spanned::Spanned;
use syn::{parse_macro_input, DeriveInput, Path};

fn get_path(crate_name: &str, span: Span) -> Path {
    let found_crate = proc_macro_crate::crate_name(crate_name).unwrap();

    match found_crate {
        FoundCrate::Itself => Path::from(Ident::new("crate", span)),
        FoundCrate::Name(name) => Path::from(Ident::new(&name, span)),
    }
}

#[proc_macro_derive(SCEvent)]
pub fn sc_event_drive(input: TokenStream) -> TokenStream {
    let input = input.clone();
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;

    let path = get_path("sc_eventbus", name.span());

    TokenStream::from(quote! {
        impl #path::events::SCEventTrait for #name {
            fn cancellable() -> bool {
                false
            }
        }
    })
}

#[proc_macro_derive(SCCancellableEvent)]
pub fn sc_cancellable_event_drive(input: TokenStream) -> TokenStream {
    let input = input.clone();
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;

    let path = get_path("sc_eventbus", name.span());

    TokenStream::from(quote! {
        impl #path::events::SCEventTrait for #name {
            fn cancellable() -> bool {
                true
            }
        }
    })
}

#[proc_macro_derive(SCEnumEvents)]
pub fn enum_events_drive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::ItemEnum);
    let span = input.span();
    let name = &input.ident;

    // Collect into Vec<TokenStream>.
    let fields = input
        .variants
        .iter()
        .map(|variant| variant.ident.clone())
        .collect::<Vec<_>>();
    // Match structs or enums.

    let ecs_path = get_path("sc_ecs", span);
    let path = get_path("sc_eventbus", span);

    let expended = quote! {
        impl #path::events::SCEnumEvents for #name {
            fn add_events(app: &#ecs_path::app::App) {
                #(
                    app.add_event::<#path::recv::SCEvent<#fields>>();
                )*
            }
        }
    };

    TokenStream::from(expended)
}
