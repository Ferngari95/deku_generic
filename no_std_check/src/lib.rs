//! `#![no_std]` + `alloc` smoke test for the code `deku_generic` generates.
//!
//! Built for a bare-metal target (`thumbv6m-none-eabi`) in CI, so any
//! accidental `std::` path in the macro output fails the build.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::marker::PhantomData;

use deku::prelude::*;
use deku_generic::{deku_generic, impl_deku_read, impl_deku_write};

pub struct Unvalidated;
pub struct Validated;

#[deku_generic(read(Frame<Unvalidated>))]
#[deku(endian = "big")]
pub struct Frame<T> {
    #[deku(bits = 4)]
    pub kind: u8,
    #[deku(bits = 4, update = "self.data.len()")]
    pub len: u8,
    #[deku(count = "len")]
    pub data: Vec<u8>,
    #[deku(assert_eq = "0xAB")]
    pub trailer: u8,
    _state: PhantomData<T>,
}

impl_deku_write!(Frame<Unvalidated>);
impl_deku_write!(Frame<Validated>);

#[deku_generic]
#[deku(ctx = "n: usize")]
pub struct WithCtx<T, const N: usize> {
    #[deku(count = "n")]
    pub head: Vec<u8>,
    pub fixed: [u8; N],
    _state: PhantomData<T>,
}

impl_deku_read!(WithCtx<Validated, 4>);
impl_deku_write!(WithCtx<Validated, 4>);

/// Parses a frame, moves it to the `Validated` state and writes it back out.
pub fn round_trip(bytes: &[u8]) -> Result<Vec<u8>, DekuError> {
    let (_, frame) = Frame::<Unvalidated>::from_bytes((bytes, 0))?;
    let validated = Frame::<Validated> {
        kind: frame.kind,
        len: frame.len,
        data: frame.data,
        trailer: frame.trailer,
        _state: PhantomData,
    };
    let mut validated = validated;
    validated.update()?;
    validated.to_bytes()
}
