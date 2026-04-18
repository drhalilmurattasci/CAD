//! `#[derive(Reflect)]` — the editor-facing reflection macro.
//!
//! Emits an `impl engine::reflection::Reflect for $Ty` that
//! describes the struct's primitive fields. The inspector, scene
//! serializer, and script bridges all consume this information
//! without needing per-type hand-rolled visitors.
//!
//! Supported field types (mapped to `ValueKind`):
//!   - `bool`                              → `ValueKind::Bool`
//!   - `i8`/`i16`/`i32`/`i64`/`u8`/`u16`/`u32` → `ValueKind::I64`
//!   - `f32`/`f64`                         → `ValueKind::F64`
//!   - `String` / `&'static str`           → `ValueKind::String`
//!
//! Fields marked `#[reflect(skip)]` are omitted from the descriptor
//! list. Unknown field types trigger a compile error directing the
//! author to either use a supported type or add `skip`.
//!
//! The derive intentionally keeps static `&[FieldDescriptor]` storage
//! so `Reflect::fields()` has the same shape as the hand-written
//! impls in `engine::reflection::tests`.

extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, Data, DeriveInput, Fields, Ident, LitStr, Token, Type, TypePath,
};

#[proc_macro_derive(Reflect, attributes(reflect))]
pub fn derive_reflect(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(input)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Arguments the user can pass to the `#[reflect(...)]` attribute on
/// individual fields. Extendable as we learn what the inspector needs.
#[derive(Default)]
struct ReflectFieldAttrs {
    skip: bool,
    /// Optional overridden name for the rendered field. Handy when the
    /// Rust field name and the authoring name diverge.
    rename: Option<String>,
}

enum ReflectFieldArg {
    Skip,
    Rename(String),
}

impl Parse for ReflectFieldArg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        match ident.to_string().as_str() {
            "skip" => Ok(Self::Skip),
            "rename" => {
                let _: Token![=] = input.parse()?;
                let lit: LitStr = input.parse()?;
                Ok(Self::Rename(lit.value()))
            }
            other => Err(syn::Error::new(
                ident.span(),
                format!("unknown #[reflect] argument `{other}` — expected `skip` or `rename`"),
            )),
        }
    }
}

fn parse_field_attrs(field: &syn::Field) -> syn::Result<ReflectFieldAttrs> {
    let mut out = ReflectFieldAttrs::default();
    for attr in &field.attrs {
        if !attr.path().is_ident("reflect") {
            continue;
        }
        let args = attr.parse_args_with(
            syn::punctuated::Punctuated::<ReflectFieldArg, Token![,]>::parse_terminated,
        )?;
        for arg in args {
            match arg {
                ReflectFieldArg::Skip => out.skip = true,
                ReflectFieldArg::Rename(name) => out.rename = Some(name),
            }
        }
    }
    Ok(out)
}

fn value_kind_for(ty: &Type) -> Option<TokenStream2> {
    let Type::Path(TypePath { path, qself: None }) = ty else {
        return None;
    };
    let last = path.segments.last()?;
    let ident = last.ident.to_string();
    let kind = match ident.as_str() {
        "bool" => quote!(::engine::reflection::ValueKind::Bool),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" => {
            quote!(::engine::reflection::ValueKind::I64)
        }
        "f32" | "f64" => quote!(::engine::reflection::ValueKind::F64),
        "String" | "str" => quote!(::engine::reflection::ValueKind::String),
        _ => return None,
    };
    Some(kind)
}

fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let ty_name = input.ident.clone();
    let ty_name_str = ty_name.to_string();

    let data = match input.data {
        Data::Struct(d) => d,
        _ => {
            return Err(syn::Error::new_spanned(
                &ty_name,
                "#[derive(Reflect)] only supports structs for now",
            ));
        }
    };

    let named = match data.fields {
        Fields::Named(fields) => fields.named,
        Fields::Unit => Default::default(),
        Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                &ty_name,
                "#[derive(Reflect)] requires named fields (tuple structs not supported)",
            ));
        }
    };

    let mut descriptors = Vec::new();
    for field in named {
        let attrs = parse_field_attrs(&field)?;
        if attrs.skip {
            continue;
        }
        let field_ident = field
            .ident
            .clone()
            .expect("named fields checked above");
        let rendered_name = attrs
            .rename
            .unwrap_or_else(|| field_ident.to_string());
        let Some(kind_tokens) = value_kind_for(&field.ty) else {
            return Err(syn::Error::new_spanned(
                &field.ty,
                format!(
                    "#[derive(Reflect)]: field `{}` has unsupported type. Use a \
                     primitive (bool/i64/f64/String) or annotate with \
                     `#[reflect(skip)]`.",
                    field_ident
                ),
            ));
        };
        descriptors.push(quote!(
            ::engine::reflection::FieldDescriptor {
                name: #rendered_name,
                kind: #kind_tokens,
            }
        ));
    }

    let field_count = descriptors.len();
    let fields_static = format_ident!("__REFLECT_FIELDS_{}", ty_name);

    let expanded = quote! {
        const _: () = {
            static #fields_static: [
                ::engine::reflection::FieldDescriptor; #field_count
            ] = [#( #descriptors ),*];

            impl ::engine::reflection::Reflect for #ty_name {
                fn type_name() -> &'static str {
                    #ty_name_str
                }

                fn fields() -> &'static [::engine::reflection::FieldDescriptor] {
                    &#fields_static
                }
            }
        };
    };
    Ok(expanded)
}
