use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use proc_macro_crate::FoundCrate;
use quote::quote;
use syn::spanned::Spanned;
use syn::{parse_macro_input, Path};

fn get_path(crate_name: &str, span: Span) -> Path {
    let found_crate = proc_macro_crate::crate_name(crate_name).unwrap();

    match found_crate {
        FoundCrate::Itself => Path::from(Ident::new("crate", span)),
        FoundCrate::Name(name) => Path::from(Ident::new(&name, span)),
    }
}

#[proc_macro_derive(SCPluginSchedule)]
pub fn plugin_schedule_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let span = input.span();
    let name = input.ident;
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();
    let ecs_path = get_path("sc_ecs", span);
    let plugin_path = get_path("sc_plugin", span);
    TokenStream::from(quote! {
        impl #impl_generics #ecs_path::schedule::ScheduleLabel for #name #type_generics #where_clause {
            fn dyn_clone(&self) -> ::std::boxed::Box<dyn #ecs_path::schedule::ScheduleLabel> {
                ::std::boxed::Box::new(::std::clone::Clone::clone(self))
            }

            fn as_dyn_eq(&self) -> &dyn #ecs_path::dyn_method::DynEq {
                self
            }

            fn dyn_hash(&self, mut state: &mut dyn ::std::hash::Hasher) {
                let ty_id = ::std::any::TypeId::of::<Self>();
                ::std::hash::Hash::hash(&ty_id, &mut state);
                ::std::hash::Hash::hash(self, &mut state);
            }
        }

        impl #impl_generics #plugin_path::schedule::SCPluginSchedule for #name #type_generics #where_clause {}
    })
}

#[proc_macro_derive(SCPlugin)]
pub fn plugin_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let span = input.span();
    let name = input.ident;
    let plugin_path = get_path("sc_plugin", span);
    // Keep source compatibility for existing in-process plugins. This derive
    // only verifies the trait implementation; it intentionally emits no ABI
    // symbol because Rust trait objects are not a stable FFI representation.
    TokenStream::from(quote! {
        const _: fn() = || {
            fn assert_sc_plugin<T: #plugin_path::SCPlugin>() {}
            assert_sc_plugin::<#name>();
        };
    })
}
