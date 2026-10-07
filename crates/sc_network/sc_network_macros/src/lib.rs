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

#[proc_macro_derive(MinecraftPackets)]
pub fn minecraft_packets_drive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::ItemEnum);

    let span = input.span();
    let name = &input.ident;

    // Collect variant idents into a Vec.
    let fields = input
        .variants
        .iter()
        .map(|variant| variant.ident.clone())
        .collect::<Vec<_>>();
    // Resolve crate paths for the generated code.

    let path = get_path("sc_network", span);
    let ecs_path = get_path("sc_ecs", span);

    // Each variant discriminant comes from the explicit integer discriminant of `#[repr(u16)]`.
    // It must match the first two bytes written by `BinaryIo`; otherwise `packet_id()`
    // disagrees with the wire bytes (covered by the cross-check test in `tests/packet_id.rs`).
    let ids: Option<Vec<u16>> = input
        .variants
        .iter()
        .map(|variant| {
            // syn 2.x: `discriminant: Option<(Eq, Expr)>`
            literal_discriminant(variant.discriminant.as_ref().map(|(_, expr)| expr))
        })
        .collect();

    let packet_types: Option<Vec<syn::Type>> = input
        .variants
        .iter()
        .map(|variant| {
            let syn::Fields::Unnamed(fields) = &variant.fields else {
                return None;
            };
            if fields.unnamed.len() != 1 {
                return None;
            }
            fields.unnamed.first().map(|field| field.ty.clone())
        })
        .collect();

    // Skip generating packet_id() if any variant lacks an explicit integer discriminant;
    // callers fall back to reading the first two serialized bytes.
    let id_impl = match (&ids, &packet_types) {
        (Some(ids), Some(packet_types)) if ids.len() == packet_types.len() => {
            let packet_count = ids.len();
            quote! {
                /// Packet id: the discriminant value as a compile-time constant.
                ///
                /// Serializing the whole packet just to read two bytes is expensive
                /// (a `LevelChunk` can reach 74KB). The send path looks the id up
                /// directly from the concrete packet type instead.
                #[inline]
                pub const fn packet_id(&self) -> u16 {
                    match self {
                        #(Self::#fields(..) => #ids,)*
                    }
                }

                /// Variant name as a compile-time constant string.
                ///
                /// Formatting the whole packet via Debug just for the name is
                /// expensive for large payloads, so the name is a constant.
                #[inline]
                pub const fn packet_name(&self) -> &'static str {
                    match self {
                        #(Self::#fields(..) => stringify!(#fields),)*
                    }
                }

                /// All `(variant name, packet id)` pairs for tests and diagnostics.
                pub const PACKET_IDS: &'static [(&'static str, u16)] = &[
                    #( (stringify!(#fields), #ids), )*
                ];

                /// Resolve a concrete packet type to its wire id without building
                /// or cloning the payload-bearing `MinecraftPackets` enum.
                pub fn packet_id_for_type<T: 'static>() -> Option<u16> {
                    static IDS: ::std::sync::OnceLock<
                        ::std::collections::HashMap<::std::any::TypeId, u16>
                    > = ::std::sync::OnceLock::new();
                    let ids = IDS.get_or_init(|| {
                        let mut ids = ::std::collections::HashMap::with_capacity(#packet_count);
                        #(
                            ids.insert(::std::any::TypeId::of::<#packet_types>(), #ids);
                        )*
                        ids
                    });
                    ids.get(&::std::any::TypeId::of::<T>()).copied()
                }
            }
        }
        _ => quote! {
            /// This enum has no compile-time packet id because some variants
            /// lack explicit integer discriminants.
            pub fn packet_id_for_type<T: 'static>() -> Option<u16> {
                None
            }
        },
    };

    let expended = quote! {
        impl #name {
            #id_impl

            pub fn send_event(self, world: &#ecs_path::world::World, entity: #ecs_path::entity::EntityId, timestamp: u128) {
                match self {
                    #(
                        #name::#fields(packet) => {
                            <#fields as #path::protocol::MinecraftPacket>::send_event(packet, world, entity, timestamp);
                        }
                    ),*
                }
            }

            pub fn add_events(app: &#ecs_path::app::App) {
                #(
                    app.add_event::<#path::protocol::recv::MinecraftPacketReceiver<#fields>>();
                )*
            }
        }
    };

    TokenStream::from(expended)
}

/// Parse an integer literal from a discriminant expression (supports hex/decimal/negation).
/// Non-literal forms (constant refs, expressions, missing) return None.
fn literal_discriminant(expr: Option<&syn::Expr>) -> Option<u16> {
    let expr = expr?;
    match expr {
        syn::Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Int(value) => value.base10_parse::<u16>().ok(),
            _ => None,
        },
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => {
            // Negative discriminants wrap to u16 two's complement.
            let magnitude = literal_discriminant(Some(&unary.expr))?;
            Some(0u16.wrapping_sub(magnitude))
        }
        _ => None,
    }
}

#[proc_macro_derive(MinecraftPacket)]
pub fn minecraft_packet_drive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let span = input.span();
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();

    let path = get_path("sc_network", span);
    let ecs_path = get_path("sc_ecs", span);

    TokenStream::from(quote! {
        impl #impl_generics #path::protocol::MinecraftPacket for #name #type_generics #where_clause {
            fn send_event(self, world: &#ecs_path::world::World, entity: #ecs_path::entity::EntityId, timestamp: u128) {
                world.send_event(#path::protocol::recv::MinecraftPacketReceiver::new(entity, timestamp, self));
            }

            fn to_packets(self) -> #path::protocol::MinecraftPackets {
                #path::protocol::MinecraftPackets::#name(self)
            }
        }
    })
}
