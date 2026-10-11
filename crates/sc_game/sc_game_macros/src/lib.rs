//! Proc macros for `sc_game` network intents.
//!
//! Mirrors `sc_network_macros` (`MinecraftPacket` / `MinecraftPackets`):
//! - `#[derive(GameIntent)]` on a payload struct implements
//!   [`IntentPayload`](crate::net::intent::IntentPayload) from
//!   `#[intent(...)]` attributes;
//! - `#[derive(GameIntents)]` on the `NetworkIntent` enum generates
//!   `kind()`, `reliability()`, `entity_id()`, `is_overwritable_state()`
//!   and `INTENT_NAMES` by delegating to the payload trait.
//!
//! Queue policy (coalescing keys, lifecycle barriers) intentionally stays
//! in `net::outbox`: it is a queue concern, not an intent-fact concern.
//! Payloads that need data-dependent reliability (e.g. `MovePlayer`
//! Normal vs Teleport/Reset) skip the struct derive and implement
//! `IntentPayload` manually.

use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use quote::quote;
use syn::spanned::Spanned;
use syn::{parse_macro_input, DeriveInput, Lit, Meta, Path};

fn sc_game_path(span: Span) -> Path {
    match proc_macro_crate::crate_name("sc_game") {
        Ok(proc_macro_crate::FoundCrate::Itself) => Path::from(Ident::new("crate", span)),
        Ok(proc_macro_crate::FoundCrate::Name(name)) => Path::from(Ident::new(&name, span)),
        Err(_) => Path::from(Ident::new("crate", span)),
    }
}

/// Parse `#[intent(reliability = "state"|"fact", entity = "<field>"|"none")]`.
fn parse_intent_attr(input: &DeriveInput) -> (bool, Option<String>) {
    let mut is_state = false;
    let mut entity_field: Option<String> = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("intent") {
            continue;
        }
        let Ok(list) = attr.meta.require_list() else {
            continue;
        };
        let Ok(items) = list.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        ) else {
            continue;
        };
        for item in items {
            let Meta::NameValue(pair) = item else {
                continue;
            };
            if pair.path.is_ident("reliability") {
                if let syn::Expr::Lit(expr) = &pair.value {
                    if let Lit::Str(s) = &expr.lit {
                        is_state = s.value() == "state";
                    }
                }
            } else if pair.path.is_ident("entity") {
                if let syn::Expr::Lit(expr) = &pair.value {
                    if let Lit::Str(s) = &expr.lit {
                        let v = s.value();
                        if v != "none" {
                            entity_field = Some(v);
                        }
                    }
                }
            }
        }
    }
    (is_state, entity_field)
}

#[proc_macro_derive(GameIntent, attributes(intent))]
pub fn game_intent_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let span = input.span();
    let name = &input.ident;
    let (generics_impl, generics_ty, generics_where) = &input.generics.split_for_impl();
    let path = sc_game_path(span);
    let (is_state, entity_field) = parse_intent_attr(&input);

    let reliability = if is_state {
        quote! { #path::net::intent::IntentReliability::OverwritableState }
    } else {
        quote! { #path::net::intent::IntentReliability::ReliableFact }
    };
    let entity_body = match entity_field {
        Some(field) => {
            let field = Ident::new(&field, span);
            quote! { Some(self.#field) }
        }
        None => quote! { None },
    };

    TokenStream::from(quote! {
        impl #generics_impl #path::net::intent::IntentPayload for #name #generics_ty #generics_where {
            fn reliability(&self) -> #path::net::intent::IntentReliability {
                #reliability
            }
            fn entity_id(&self) -> Option<u64> {
                #entity_body
            }
            fn into_intent(self) -> #path::net::intent::NetworkIntent {
                #path::net::intent::NetworkIntent::#name(self)
            }
        }
    })
}

#[proc_macro_derive(GameIntents)]
pub fn game_intents_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::ItemEnum);
    let span = input.span();
    let name = &input.ident;
    let path = sc_game_path(span);

    let variants: Vec<Ident> = input.variants.iter().map(|v| v.ident.clone()).collect();

    TokenStream::from(quote! {
        impl #name {
            /// Intent variant name as a compile-time constant string.
            #[inline]
            pub fn kind(&self) -> &'static str {
                match self {
                    #(Self::#variants(..) => stringify!(#variants),)*
                }
            }

            /// All intent variant names for diagnostics and tests.
            pub const INTENT_NAMES: &'static [&'static str] = &[
                #(stringify!(#variants),)*
            ];

            /// Intent category: overwriteable state vs lossless reliable facts.
            #[inline]
            pub fn reliability(&self) -> #path::net::intent::IntentReliability {
                match self {
                    #(Self::#variants(payload) => #path::net::intent::IntentPayload::reliability(payload),)*
                }
            }

            /// Whether budget shortfall may safely ignore it.
            #[inline]
            pub fn is_overwritable_state(&self) -> bool {
                self.reliability() == #path::net::intent::IntentReliability::OverwritableState
            }

            /// Target entity runtime id of the intent (None without a target).
            #[inline]
            pub fn entity_id(&self) -> Option<u64> {
                match self {
                    #(Self::#variants(payload) => #path::net::intent::IntentPayload::entity_id(payload),)*
                }
            }
        }
    })
}
