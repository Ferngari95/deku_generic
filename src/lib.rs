//! deku's traits for particular instantiations of a generic struct.
//!
//! `#[derive(DekuRead)]` on `struct Foo<T>` produces
//! `impl<T> DekuReader for Foo<T>`. If `T` is a typestate marker that is
//! usually more than wanted: only `Foo<Unvalidated>` should be readable off
//! the wire, while both `Foo<Unvalidated>` and `Foo<Validated>` may need
//! writing. This crate lets the impls be requested one instantiation at a
//! time.
//!
//! ```rust
//! use core::marker::PhantomData;
//! use deku::prelude::*;
//! use deku_generic::{deku_generic, impl_deku_read, impl_deku_write};
//!
//! pub struct Unvalidated;
//! pub struct Validated;
//!
//! #[deku_generic]
//! #[derive(Debug, PartialEq)]
//! pub struct Foo<T> {
//!     #[deku(bits = 4)]
//!     kind: u8,
//!     #[deku(bits = 4)]
//!     len: u8,
//!     #[deku(count = "len")]
//!     data: Vec<u8>,
//!     _state: PhantomData<T>,
//! }
//!
//! impl_deku_read!(Foo<Unvalidated>);
//! impl_deku_write!(Foo<Unvalidated>);
//! impl_deku_write!(Foo<Validated>);
//!
//! let bytes = [0x12, 0xaa, 0xbb];
//! let (_, foo) = Foo::<Unvalidated>::from_bytes((&bytes, 0)).unwrap();
//! assert_eq!(foo.data, vec![0xaa, 0xbb]);
//! assert_eq!(foo.to_bytes().unwrap(), bytes);
//!
//! let validated = Foo::<Validated> { kind: 1, len: 2, data: vec![0xaa, 0xbb], _state: PhantomData };
//! assert_eq!(validated.to_bytes().unwrap(), bytes);
//! ```
//!
//! `Foo::<Validated>::from_bytes(..)` does not exist:
//!
//! ```rust,compile_fail
//! use core::marker::PhantomData;
//! use deku::prelude::*;
//! use deku_generic::{deku_generic, impl_deku_read};
//!
//! pub struct Unvalidated;
//! pub struct Validated;
//!
//! #[deku_generic(read(Foo<Unvalidated>))]
//! pub struct Foo<T> {
//!     a: u8,
//!     _state: PhantomData<T>,
//! }
//!
//! let _ = Foo::<Validated>::from_bytes((&[1u8], 0));
//! ```
//!
//! # Usage
//!
//! Put [`deku_generic`] on the struct in place of `#[derive(DekuRead, DekuWrite)]`
//! and keep the `#[deku(...)]` attributes as they are; the attribute records
//! them and strips them from the real struct. Then ask for impls with
//! [`impl_deku_read!`], [`impl_deku_write!`] or [`impl_deku_read_write!`],
//! or list them on the attribute itself:
//!
//! ```rust,ignore
//! #[deku_generic(read(Foo<Unvalidated>), write(Foo<Unvalidated>, Foo<Validated>))]
//! pub struct Foo<T> { .. }
//! ```
//!
//! `read` produces `DekuReader`, `DekuContainerRead` and `TryFrom<&[u8]>`.
//! `write` produces `DekuWriter`, `DekuContainerWrite`, `DekuUpdate` and
//! `TryFrom<Foo<..>> for Vec<u8>`. The container and `TryFrom` impls are
//! only produced when deku's derive would produce them, i.e. without a `ctx`
//! or with a `ctx_default`.
//!
//! Type parameters (with defaults, so `impl_deku_read!(Foo)` is fine if
//! `T` has one), const parameters (`impl_deku_read!(Buf<16>)`) and lifetime
//! parameters are all accepted. Lifetimes stay generic in the impl and may be
//! written or left out: `Foo<'a, X>` and `Foo<X>` mean the same thing.
//!
//! A `PhantomData<..>` field with no `#[deku]` attribute is treated as
//! `#[deku(skip)]`; deku has no impl for `PhantomData`.
//!
//! The generated code refers to this crate as `::deku_generic`. If it is
//! only reachable under another name, say so on the attribute:
//! `#[deku_generic(crate = "my_reexports::deku_generic")]`.
//!
//! # `no_std`
//!
//! The crate is `#![no_std]`. The generated code needs `alloc` for
//! `to_bytes` and the `Vec<u8>` conversion, so use deku without default
//! features:
//!
//! ```toml
//! [dependencies]
//! deku = { version = "0.20", default-features = false, features = ["alloc"] }
//! deku_generic = { git = "https://github.com/Ferngari95/deku_generic" }
//! ```
//!
//! Add deku's `bits` feature for `bits = ..` attributes. CI builds a check
//! crate for `thumbv6m-none-eabi`.
//!
//! `deku_generic` 0.1 depends on deku 0.20 and the generated impls use that
//! copy, so Cargo keeps the two in step: a deku of another major version in
//! your own `Cargo.toml` shows up as a version mismatch rather than as an
//! unexplained missing trait. Edition 2024, MSRV 1.85.
//!
//! # How it works
//!
//! Each requested instantiation expands to an anonymous const block. It
//! contains an alias per generic parameter (`type T = Unvalidated;`), a
//! hidden non-generic copy of the struct with the same fields and
//! `#[deku(...)]` attributes, and deku's own derive applied to that copy.
//! The impls for `Foo<Unvalidated>` delegate to it: reading moves the copy's
//! fields into `Foo`, writing builds a copy whose fields borrow `&self`,
//! which deku can write because it implements `DekuWriter` for `&T`. deku
//! evaluates all attributes itself, so their semantics are whatever deku's
//! are. The impls name deku through a hidden re-export from this crate's own
//! deku dependency; the derive on the copy is deku's and finds deku by
//! itself.
//!
//! deku exposes sibling fields to write-side expressions (`cond`, `assert`,
//! `assert_eq`, `ctx`, `writer`, ..) as `&T` locals named after the field.
//! On the borrowing copy those would be `&&T`, so expressions that mention
//! a field are wrapped in a block that rebinds it to `&T` first. Expressions
//! written for a plain `#[derive(DekuWrite)]` work as they are.
//!
//! # Limitations
//!
//! * Structs only, no enums.
//! * `#[deku(temp)]` fields are rejected.
//! * The `impl_deku_*!` macros have to be invoked in the module that defines
//!   the struct, or with a full path (`crate::proto::Foo<X>`) from a place
//!   that can see the fields. The attribute form has no such restriction.
//! * Field types must not mention `Self`, and concrete arguments must not
//!   mention lifetimes other than `'static`.
//! * `deku` still has to be a dependency of your crate, under that name:
//!   deku's own derive runs on the hidden copy, and its output names `deku`.
//! * Writing goes through deku's `impl DekuWriter<Ctx> for &T`, which wants
//!   `Ctx: Copy`. All of deku's own ctx types are.
//! * Structs with lifetime parameters work as far as deku's derive does; in
//!   deku 0.20 that means they need a `ctx`.

#![no_std]

pub use deku_generic_macros::{
    deku_generic, impl_deku_read, impl_deku_read_write, impl_deku_write,
};

#[doc(hidden)]
pub use deku_generic_macros::__deku_generic_impl;

/// Used by the generated code. Not public API.
#[doc(hidden)]
pub mod __private {
    pub use deku;
}

// Runs the README's code blocks as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}
