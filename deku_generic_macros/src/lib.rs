//! Proc-macro implementation for the `deku_generic` crate.
//!
//! Do not depend on this crate directly; use `deku_generic` instead.

mod attrs;
mod expand;

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, ItemStruct, Path, Token, parenthesized, parse_macro_input};

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
                format!("unknown option `{other}`; expected `read`, `write` or `read_write`"),
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

/// The full argument list of `#[deku_generic(...)]`.
struct AttrArgs {
    groups: Punctuated<AttrGroup, Token![,]>,
}

impl Parse for AttrArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(AttrArgs {
            groups: input.parse_terminated(AttrGroup::parse, Token![,])?,
        })
    }
}

/// Input of the hidden `__deku_generic_impl!` macro:
/// `@read path::Foo<A, B> ; struct Foo<T, U> { .. }`
struct ImplInput {
    modes: Vec<Mode>,
    target: Path,
    item: ItemStruct,
}

impl Parse for ImplInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        input.parse::<Token![@]>()?;
        let mode: Ident = input.parse()?;
        let modes = Mode::from_ident(&mode)?;
        let target: Path = input.parse()?;
        input.parse::<Token![;]>()?;
        let item: ItemStruct = input.parse()?;
        Ok(ImplInput {
            modes,
            target,
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
    let args = parse_macro_input!(attr as AttrArgs);
    let item = parse_macro_input!(item as ItemStruct);

    let mut requests: Vec<(Mode, Path)> = Vec::new();
    for group in args.groups {
        for target in group.targets {
            for mode in &group.modes {
                requests.push((*mode, target.clone()));
            }
        }
    }

    expand::attribute(&item, &requests)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
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
        item,
    } = parse_macro_input!(input as ImplInput);

    let mut out = proc_macro2::TokenStream::new();
    for mode in modes {
        match expand::instantiate(mode, &target, &item) {
            Ok(ts) => out.extend(ts),
            Err(e) => return e.into_compile_error().into(),
        }
    }
    out.into()
}

/// Path to the `deku` crate as seen from the user's crate.
pub(crate) fn deku_path() -> proc_macro2::TokenStream {
    crate_path("deku")
}

/// Path to the `deku_generic` crate as seen from the user's crate.
pub(crate) fn self_path() -> proc_macro2::TokenStream {
    crate_path("deku_generic")
}

fn crate_path(name: &str) -> proc_macro2::TokenStream {
    use proc_macro_crate::{FoundCrate, crate_name};
    let ident = match crate_name(name) {
        Ok(FoundCrate::Name(renamed)) => format_ident!("{}", renamed),
        // `Itself` also covers integration tests and doctests of the crate
        // itself, where it is still reachable under its own name.
        Ok(FoundCrate::Itself) | Err(_) => format_ident!("{}", name),
    };
    quote!(::#ident)
}
