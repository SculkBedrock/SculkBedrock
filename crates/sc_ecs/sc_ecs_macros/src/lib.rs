use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use proc_macro_crate::FoundCrate;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::token::Comma;
use syn::{parse_macro_input, LitInt, Path, Result};

fn get_path(crate_name: &str, span: Span) -> Path {
    let found_crate = proc_macro_crate::crate_name(crate_name).unwrap();

    match found_crate {
        FoundCrate::Itself => Path::from(Ident::new("crate", span)),
        FoundCrate::Name(name) => Path::from(Ident::new(&name, span)),
    }
}

struct AllTuples {
    macro_ident: Ident,
    start: usize,
    end: usize,
    idents: Vec<Ident>,
}

impl Parse for AllTuples {
    fn parse(input: ParseStream) -> Result<Self> {
        let macro_ident = input.parse::<Ident>()?;
        input.parse::<Comma>()?;
        let start = input.parse::<LitInt>()?.base10_parse()?;
        input.parse::<Comma>()?;
        let end = input.parse::<LitInt>()?.base10_parse()?;
        input.parse::<Comma>()?;
        let mut idents = vec![input.parse::<Ident>()?];
        while input.parse::<Comma>().is_ok() {
            idents.push(input.parse::<Ident>()?);
        }

        Ok(AllTuples {
            macro_ident,
            start,
            end,
            idents,
        })
    }
}

#[proc_macro]
pub fn all_tuples(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as AllTuples);
    if input.start > input.end {
        return syn::Error::new(
            input.macro_ident.span(),
            "all_tuples start must be less than or equal to end",
        )
        .to_compile_error()
        .into();
    }

    // `i` is the tuple arity. Generate exactly `end` identifiers so every
    // requested invocation can safely take the first `i` identifiers.
    let mut ident_tuples = Vec::with_capacity(input.end);

    for i in 0..input.end {
        let idents = input
            .idents
            .iter()
            .map(|ident| format_ident!("{}{}", ident, i));

        if input.idents.len() < 2 {
            ident_tuples.push(quote! {
                #(#idents)*
            });
        } else {
            ident_tuples.push(quote! {
                (#(#idents),*)
            });
        }
    }

    let macro_ident = &input.macro_ident;
    let invocations = (input.start..=input.end).map(|arity| {
        let ident_tuples = &ident_tuples[..arity];
        quote! {
            #macro_ident!(#(#ident_tuples),*);
        }
    });

    TokenStream::from(quote! {
        #(
            #invocations
        )*
    })
}

#[proc_macro_attribute]
pub fn async_system(attr: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::ItemFn);
    let attr = if !attr.is_empty() {
        parse_macro_input!(attr as syn::LitStr).value()
    } else {
        return syn::Error::new_spanned(
            &input.sig.fn_token,
            "async_system requires an explicit mode; use \"block_on\" or spawn tasks manually",
        )
        .to_compile_error()
        .into();
    };
    if attr != "block_on" {
        return syn::Error::new_spanned(
            &input.sig.fn_token,
            "async_system only supports \"block_on\"; spawn mode can move ECS guards across await",
        )
        .to_compile_error()
        .into();
    }
    let attr = Ident::new(&attr, Span::call_site());
    let is_async = input.sig.asyncness.is_some();
    if !is_async {
        return syn::Error::new_spanned(
            &input.sig.fn_token,
            "async_system can only be used on async functions",
        )
        .to_compile_error()
        .into();
    }

    let ecs_path = get_path("sc_ecs", input.span());

    let mut new_input = input.clone();
    new_input.sig.asyncness = None;
    let async_code = new_input.block;
    let new_code = quote! {
        {
            let mut runtime = #ecs_path::async_manager::SCECSAsync::runtime();
            runtime.#attr(async move {
                #async_code
            });
        }
    };
    new_input.block = Box::new(syn::parse2(new_code).unwrap());
    TokenStream::from(quote! {
        #new_input
    })
}

// ---------------------------------------------------------------------------
// Resource derive: layout code -> ResourceId
//
// Layout code = repr prefix + struct name + alphabetically ordered
// `field:type` list, all normalized to a spaceless form
// (`Vec < T >` -> `Vec<T>`). It is a pure function of the source: host and
// cdylib plugins built from the same source derive the same ResourceId, so
// cross-DLL resource lookup hits, and equal IDs by themselves prove equal
// layouts (layout certificate).
// ---------------------------------------------------------------------------

/// Renders a token stream as a spaceless canonical string.
fn render_token_stream(ts: proc_macro2::TokenStream) -> String {
    use proc_macro2::TokenTree;
    let mut s = String::new();
    for tt in ts {
        match tt {
            TokenTree::Ident(i) => s.push_str(&i.to_string()),
            TokenTree::Punct(p) => s.push(p.as_char()),
            TokenTree::Literal(l) => s.push_str(&l.to_string()),
            TokenTree::Group(g) => {
                let (open, close) = match g.delimiter() {
                    proc_macro2::Delimiter::Parenthesis => ('(', ')'),
                    proc_macro2::Delimiter::Bracket => ('[', ']'),
                    proc_macro2::Delimiter::Brace => ('{', '}'),
                    proc_macro2::Delimiter::None => continue,
                };
                s.push(open);
                s.push_str(&render_token_stream(g.stream()));
                s.push(close);
            }
        }
    }
    s
}

fn render_type(ty: &syn::Type) -> String {
    let mut ts = proc_macro2::TokenStream::new();
    quote::ToTokens::to_tokens(ty, &mut ts);
    render_token_stream(ts)
}

/// `#[repr(...)]` affects the real layout and must be included in the layout code (empty prefix when absent).
fn render_repr(attrs: &[syn::Attribute]) -> String {
    for attr in attrs {
        if !attr.path().is_ident("repr") {
            continue;
        }
        if let syn::Meta::List(list) = &attr.meta {
            return format!("#[repr({})]", render_token_stream(list.tokens.clone()));
        }
    }
    String::new()
}

/// Struct layout code: named fields sorted by field name (declaration order
/// does not affect the ID); tuple structs keep positional order (position is
/// itself semantics).
fn struct_layout_code(input: &syn::DeriveInput) -> syn::Result<String> {
    let mut s = render_repr(&input.attrs);
    s.push_str(&input.ident.to_string());
    let fields = match &input.data {
        syn::Data::Struct(st) => &st.fields,
        _ => {
            return Err(syn::Error::new_spanned(
                input,
                "Resource derive only supports structs; write the impl by hand for enum/union resources",
            ))
        }
    };
    match fields {
        syn::Fields::Named(named) => {
            let mut parts: Vec<(String, String)> = named
                .named
                .iter()
                .map(|f| (f.ident.as_ref().unwrap().to_string(), render_type(&f.ty)))
                .collect();
            parts.sort_by(|a, b| a.0.cmp(&b.0));
            s.push('{');
            s.push_str(
                &parts
                    .iter()
                    .map(|(n, t)| format!("{n}:{t}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            s.push('}');
        }
        syn::Fields::Unnamed(unnamed) => {
            s.push('(');
            s.push_str(
                &unnamed
                    .unnamed
                    .iter()
                    .map(|f| render_type(&f.ty))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            s.push(')');
        }
        syn::Fields::Unit => {}
    }
    Ok(s)
}

#[proc_macro_derive(Resource)]
pub fn resource_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let span = input.span();
    let name = input.ident.clone();
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();
    let ecs_path = get_path("sc_ecs", span);

    let layout = match struct_layout_code(&input) {
        Ok(l) => l,
        Err(e) => return e.to_compile_error().into(),
    };
    let layout_lit = syn::LitStr::new(&layout, span);

    // Generic params are only placeholder tokens in the compile-time layout
    // string; the instantiated concrete type is appended as a runtime
    // `type_name` suffix (e.g. `Foo{...}<crate::Bar>`): same-source build
    // copies share the type_name, while different instantiations stay
    // distinguishable by suffix.
    let params: Vec<syn::Ident> = input
        .generics
        .type_params()
        .map(|p| p.ident.clone())
        .collect();
    let generic_suffix = if params.is_empty() {
        quote! { String::new() }
    } else {
        quote! { format!("<{}>", [#(std::any::type_name::<#params>()),*].join(",")) }
    };

    TokenStream::from(quote! {
        impl #impl_generics #ecs_path::resource::Resource for #name #type_generics #where_clause {
            fn resource_id() -> #ecs_path::resource::ResourceId {
                // The cache must be process-wide: a `static` inside a generic
                // function is not isolated per monomorphized instance
                // (observed: Events<A> and Events<B> sharing one OnceLock,
                // so the second instance returns the first instance's ID:
                // a type-confusion-grade bug).
                #ecs_path::resource::cached_resource_id_for::<Self>(
                    || format!("{}{}", #layout_lit, #generic_suffix)
                )
            }
            fn name() -> String {
                stringify!(#name).to_string()
            }
        }
    })
}

#[proc_macro_derive(Component)]
pub fn component_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let span = input.span();
    let name = input.ident;
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();
    let ecs_path = get_path("sc_ecs", span);
    TokenStream::from(quote! {
        impl #impl_generics #ecs_path::component::Component for #name #type_generics #where_clause {
            fn name() -> String {
                stringify!(#name).to_string()
            }

            fn name_static() -> Option<&'static str> {
                Some(stringify!(#name))
            }
        }
    })
}

#[proc_macro_derive(Event)]
pub fn event_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let span = input.span();
    let name = input.ident;
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();
    let ecs_path = get_path("sc_ecs", span);
    TokenStream::from(quote! {
        impl #impl_generics #ecs_path::event::Event for #name #type_generics #where_clause {}
        impl #impl_generics #ecs_path::component::Component for #name #type_generics #where_clause {
            fn name() -> String {
                stringify!(#name).to_string()
            }

            fn name_static() -> Option<&'static str> {
                Some(stringify!(#name))
            }
        }
    })
}

#[proc_macro_derive(EnumEvents)]
pub fn enum_events_drive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::ItemEnum);
    let span = input.span();
    let name = &input.ident;

    // Collect into Vec<TokenStream>
    let fields = input
        .variants
        .iter()
        .map(|variant| variant.ident.clone())
        .collect::<Vec<_>>();
    // Match struct or enum

    let ecs_path = get_path("sc_ecs", span);

    let expended = quote! {
        impl #ecs_path::event::EnumEvents for #name {
            fn send_event(self, world: &#ecs_path::world::World) -> std::option::Option<#ecs_path::event::EventId> {
                match self {
                    #(
                        Self::#fields(event) => {
                            return world.send_event::<#fields>(event);
                        }
                    )*
                }
            }
            fn add_events(app: &#ecs_path::app::App) {
                #(
                    app.add_event::<#fields>();
                )*
            }
        }
    };

    TokenStream::from(expended)
}

#[proc_macro_derive(ScheduleLabel)]
pub fn schedule_label_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let span = input.span();
    let name = input.ident;
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();
    let ecs_path = get_path("sc_ecs", span);
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
    })
}

#[proc_macro_derive(AppLabel)]
pub fn app_label_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let span = input.span();
    let name = input.ident;
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();
    let ecs_path = get_path("sc_ecs", span);
    TokenStream::from(quote! {
        impl #impl_generics #ecs_path::app_manager::AppLabel for #name #type_generics #where_clause {
            fn dyn_clone(&self) -> ::std::boxed::Box<dyn #ecs_path::app_manager::AppLabel> {
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
    })
}
