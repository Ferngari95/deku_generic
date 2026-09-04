//! Proc-macro implementation for the `deku_generic` crate.
//!
//! Do not depend on this crate directly; use `deku_generic` instead.

mod attrs;
mod expand;

use proc_macro::TokenStream;
use proc_macro2::{Group, Span, TokenTree};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, ItemStruct, LitStr, Path, Token, parenthesized, parse_macro_input};

/// Which impls are being requested for one concrete instantiation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Read,
    Write,
}

impl Mode {
    fn from_ident(ident: &Ident) -> syn::Result<Vec<Mode>> {
        match ident.to_string().as_str() {
            "read" => Ok(vec![Mode::Read]),
            "write" => Ok(vec![Mode::Write]),
            "read_write" => Ok(vec![Mode::Read, Mode::Write]),
            other => Err(syn::Error::new(
                ident.span(),
                format!(
                    "unknown option `{other}`; expected `read`, `write`, `read_write` or `crate = \"..\"`"
                ),
            )),
        }
    }
}

/// One `read(Foo<A>, Foo<B>)` group inside `#[deku_generic(...)]`.
struct AttrGroup {
    modes: Vec<Mode>,
    targets: Punctuated<Path, Token![,]>,
}

impl Parse for AttrGroup {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let ident: Ident = input.parse()?;
        let modes = Mode::from_ident(&ident)?;
        let content;
        parenthesized!(content in input);
        let targets = content.parse_terminated(Path::parse, Token![,])?;
        Ok(AttrGroup { modes, targets })
    }
}

/// The full argument list of `#[deku_generic(...)]`: any number of
/// `read(..)`/`write(..)`/`read_write(..)` groups and at most one
/// `crate = "path"`.
struct AttrArgs {
    groups: Vec<AttrGroup>,
    crate_path: Option<Path>,
}

impl Parse for AttrArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = AttrArgs {
            groups: Vec::new(),
            crate_path: None,
        };
        while !input.is_empty() {
            if input.peek(Token![crate]) {
                let kw: Token![crate] = input.parse()?;
                if args.crate_path.is_some() {
                    return Err(syn::Error::new(kw.span, "`crate` given more than once"));
                }
                input.parse::<Token![=]>()?;
                let lit: LitStr = input.parse()?;
                args.crate_path = Some(lit.parse_with(Path::parse_mod_style)?);
            } else {
                args.groups.push(input.parse()?);
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

/// Input of the hidden `__deku_generic_impl!` macro:
/// `@read path::Foo<A, B> ; ::deku_generic ; struct Foo<T, U> { .. }`
struct ImplInput {
    modes: Vec<Mode>,
    target: Path,
    crate_path: Path,
    item: ItemStruct,
}

impl Parse for ImplInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        input.parse::<Token![@]>()?;
        let mode: Ident = input.parse()?;
        let modes = Mode::from_ident(&mode)?;
        let target: Path = input.parse()?;
        input.parse::<Token![;]>()?;
        // The path was transcribed by the helper `macro_rules!`, so its tokens
        // carry that macro's hygiene context. deku's derive uses the span of
        // its own path for the locals it generates, so give it the call site.
        let crate_path = Path::parse_mod_style(input)?;
        let crate_path: Path = syn::parse2(respan(quote!(#crate_path), Span::call_site()))?;
        input.parse::<Token![;]>()?;
        let item: ItemStruct = input.parse()?;
        Ok(ImplInput {
            modes,
            target,
            crate_path,
            item,
        })
    }
}

/// Mark a generic struct so that deku impls can later be generated for
/// concrete instantiations of it.
///
/// See the `deku_generic` crate documentation for details.
#[proc_macro_attribute]
pub fn deku_generic(attr: TokenStream, item: TokenStream) -> TokenStream {
    // On any error the struct is still emitted (minus its deku attributes),
    // so the rest of the crate keeps type-checking and the user sees one
    // error instead of a cascade.
    let raw = proc_macro2::TokenStream::from(item.clone());
    let item: ItemStruct = match syn::parse(item) {
        Ok(item) => item,
        Err(e) => {
            let err = e.into_compile_error();
            return quote! { #err #raw }.into();
        }
    };

    let expanded = syn::parse::<AttrArgs>(attr).and_then(|args| {
        let mut requests: Vec<(Mode, Path)> = Vec::new();
        for group in args.groups {
            for target in group.targets {
                for mode in &group.modes {
                    requests.push((*mode, target.clone()));
                }
            }
        }
        let crate_path = args.crate_path.unwrap_or_else(default_crate_path);
        expand::attribute(&item, &requests, &crate_path)
    });

    match expanded {
        Ok(ts) => ts.into(),
        Err(e) => {
            let err = e.into_compile_error();
            let fallback = expand::fallback(&item);
            quote! { #err #fallback }.into()
        }
    }
}

/// Implement `DekuReader`, `DekuContainerRead` and `TryFrom<&[u8]>` for a
/// concrete instantiation of a `#[deku_generic]` struct.
#[proc_macro]
pub fn impl_deku_read(input: TokenStream) -> TokenStream {
    dispatch("read", input)
}

/// Implement `DekuWriter`, `DekuContainerWrite`, `DekuUpdate` and
/// `TryFrom<Foo<..>> for Vec<u8>` for a concrete instantiation of a
/// `#[deku_generic]` struct.
#[proc_macro]
pub fn impl_deku_write(input: TokenStream) -> TokenStream {
    dispatch("write", input)
}

/// Shorthand for `impl_deku_read!` followed by `impl_deku_write!`.
#[proc_macro]
pub fn impl_deku_read_write(input: TokenStream) -> TokenStream {
    dispatch("read_write", input)
}

/// Expand `impl_deku_*!(path::Foo<A>)` into a call of the helper macro that
/// `#[deku_generic]` emitted next to `Foo`.
fn dispatch(mode: &str, input: TokenStream) -> TokenStream {
    let target = parse_macro_input!(input as Path);
    let mut helper = target.clone();
    let Some(last) = helper.segments.last_mut() else {
        return syn::Error::new(Span::call_site(), "expected a type path such as `Foo<Bar>`")
            .into_compile_error()
            .into();
    };
    last.ident = expand::helper_ident(&last.ident);
    last.arguments = syn::PathArguments::None;
    let mode = Ident::new(mode, Span::call_site());
    quote! { #helper! { @#mode #target } }.into()
}

/// Internal: invoked through the helper macro emitted by `#[deku_generic]`.
#[doc(hidden)]
#[proc_macro]
pub fn __deku_generic_impl(input: TokenStream) -> TokenStream {
    let ImplInput {
        modes,
        target,
        crate_path,
        item,
    } = parse_macro_input!(input as ImplInput);

    let mut out = proc_macro2::TokenStream::new();
    for mode in modes {
        match expand::instantiate(mode, &target, &item, &crate_path) {
            Ok(ts) => out.extend(ts),
            Err(e) => return e.into_compile_error().into(),
        }
    }
    out.into()
}

fn respan(tokens: proc_macro2::TokenStream, span: Span) -> proc_macro2::TokenStream {
    tokens
        .into_iter()
        .map(|tt| match tt {
            TokenTree::Group(g) => {
                let mut group = Group::new(g.delimiter(), respan(g.stream(), span));
                group.set_span(span);
                TokenTree::Group(group)
            }
            mut other => {
                other.set_span(span);
                other
            }
        })
        .collect()
}

/// Where the generated code finds `deku_generic` unless the user said
/// otherwise with `#[deku_generic(crate = "..")]`.
fn default_crate_path() -> Path {
    syn::parse_quote!(::deku_generic)
}

/// Path to the `deku` crate: the copy `deku_generic` depends on and
/// re-exports, so that the generated impls and the traits they implement
/// always come from the same deku version.
pub(crate) fn deku_path(crate_path: &Path) -> proc_macro2::TokenStream {
    quote!(#crate_path::__private::deku)
}
