//! deku attribute coverage: every instantiation is compared against an
//! equivalent non-generic `#[derive(DekuRead, DekuWrite)]` struct.

use std::borrow::Cow;
use std::marker::PhantomData;

use deku::ctx::Endian;
use deku::prelude::*;
use deku_generic::{deku_generic, impl_deku_read, impl_deku_read_write, impl_deku_write};

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct A;
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct B;

// ---------------------------------------------------------------------------
// bit fields, count, cond, assert, assert_eq, update, padding
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, DekuRead, DekuWrite)]
#[deku(endian = "big")]
struct PlainPacket {
    #[deku(bits = 4)]
    version: u8,
    #[deku(bits = 4, assert = "*flags < 8")]
    flags: u8,
    #[deku(update = "self.payload.len()")]
    len: u16,
    #[deku(count = "len")]
    payload: Vec<u8>,
    #[deku(cond = "*flags & 1 == 1")]
    extra: Option<u32>,
    #[deku(pad_bytes_before = "1", assert_eq = "0xAB")]
    trailer: u8,
}

#[deku_generic]
#[derive(Debug, PartialEq)]
#[deku(endian = "big")]
struct Packet<T> {
    #[deku(bits = 4)]
    version: u8,
    #[deku(bits = 4, assert = "*flags < 8")]
    flags: u8,
    #[deku(update = "self.payload.len()")]
    len: u16,
    #[deku(count = "len")]
    payload: Vec<u8>,
    #[deku(cond = "*flags & 1 == 1")]
    extra: Option<u32>,
    #[deku(pad_bytes_before = "1", assert_eq = "0xAB")]
    trailer: u8,
    _state: PhantomData<T>,
}

impl_deku_read!(Packet<A>);
impl_deku_write!(Packet<A>);
impl_deku_write!(Packet<B>);

const PACKET_BYTES: &[u8] = &[
    0x21, // version 2, flags 1
    0x00, 0x03, // len
    0xaa, 0xbb, 0xcc, // payload
    0x01, 0x02, 0x03, 0x04, // extra (flags & 1)
    0x00, // pad
    0xab, // trailer
];

#[test]
fn packet_matches_plain_deku() {
    let (_, plain) = PlainPacket::from_bytes((PACKET_BYTES, 0)).unwrap();
    let (rest, generic) = Packet::<A>::from_bytes((PACKET_BYTES, 0)).unwrap();
    assert_eq!(rest, (&[][..], 0));

    assert_eq!(generic.version, plain.version);
    assert_eq!(generic.flags, plain.flags);
    assert_eq!(generic.len, plain.len);
    assert_eq!(generic.payload, plain.payload);
    assert_eq!(generic.extra, plain.extra);
    assert_eq!(generic.trailer, plain.trailer);

    assert_eq!(generic.to_bytes().unwrap(), plain.to_bytes().unwrap());
    assert_eq!(generic.to_bytes().unwrap(), PACKET_BYTES);

    // TryFrom<&[u8]> rejects trailing data, like deku.
    assert!(Packet::<A>::try_from(PACKET_BYTES).is_ok());
    let mut too_long = PACKET_BYTES.to_vec();
    too_long.push(0);
    assert!(Packet::<A>::try_from(too_long.as_slice()).is_err());
}

#[test]
fn packet_write_only_state() {
    let mut b = Packet::<B> {
        version: 2,
        flags: 0,
        len: 0,
        payload: vec![1, 2],
        extra: Some(9),
        trailer: 0xab,
        _state: PhantomData,
    };
    // DekuUpdate runs on the real struct.
    b.update().unwrap();
    assert_eq!(b.len, 2);

    // deku writes a `Some` regardless of `cond` (only `skip, cond` skips),
    // so just check we match the plain derive byte for byte.
    let mut plain = PlainPacket {
        version: 2,
        flags: 0,
        len: 0,
        payload: vec![1, 2],
        extra: Some(9),
        trailer: 0xab,
    };
    plain.update().unwrap();
    assert_eq!(b.to_bytes().unwrap(), plain.to_bytes().unwrap());
    assert_eq!(
        b.to_bytes().unwrap(),
        [0x20, 0, 2, 1, 2, 0, 0, 0, 9, 0, 0xab]
    );
}

#[test]
fn packet_write_assertions_fire() {
    let bad_trailer = Packet::<B> {
        version: 0,
        flags: 0,
        len: 0,
        payload: vec![],
        extra: None,
        trailer: 0x00,
        _state: PhantomData,
    };
    assert!(matches!(
        bad_trailer.to_bytes(),
        Err(DekuError::Assertion(_))
    ));

    let bad_flags = Packet::<B> {
        flags: 9,
        trailer: 0xab,
        ..bad_trailer
    };
    assert!(matches!(bad_flags.to_bytes(), Err(DekuError::Assertion(_))));
}

// ---------------------------------------------------------------------------
// generic parameter used in real fields, tuple struct, where clause
// ---------------------------------------------------------------------------

#[deku_generic]
#[derive(Debug, PartialEq)]
#[deku(endian = "little")]
struct Pair<T, U>(#[deku(bits = 12)] T, #[deku(bits = 4)] U)
where
    T: Copy;

impl_deku_read_write!(Pair<u16, u8>);

#[derive(Debug, PartialEq, DekuRead, DekuWrite)]
#[deku(endian = "little")]
struct PlainPair(#[deku(bits = 12)] u16, #[deku(bits = 4)] u8);

#[test]
fn tuple_struct_with_generic_fields() {
    let bytes = [0x12, 0x34];
    let (_, plain) = PlainPair::from_bytes((&bytes, 0)).unwrap();
    let (_, p) = Pair::<u16, u8>::from_bytes((&bytes, 0)).unwrap();
    assert_eq!((p.0, p.1), (plain.0, plain.1));
    assert_eq!(p.1, 4);
    assert_eq!(p.to_bytes().unwrap(), bytes);
}

#[deku_generic]
#[derive(Debug, PartialEq)]
struct Items<T> {
    count: u8,
    #[deku(count = "count")]
    items: Vec<T>,
}

impl_deku_read_write!(Items<u16>);
impl_deku_read_write!(Items<u8>);

#[test]
fn vec_of_generic() {
    let bytes = [2, 0x01, 0x02, 0x03, 0x04];
    let (_, i16) = Items::<u16>::from_bytes((&bytes, 0)).unwrap();
    assert_eq!(i16.items, vec![0x0201, 0x0403]);
    assert_eq!(i16.to_bytes().unwrap(), bytes);

    let (rest, i8) = Items::<u8>::from_bytes((&bytes, 0)).unwrap();
    assert_eq!(i8.items, vec![1, 2]);
    assert_eq!(rest.0, &[3, 4]);
}

// ---------------------------------------------------------------------------
// const generics and defaults
// ---------------------------------------------------------------------------

#[deku_generic]
#[derive(Debug, PartialEq)]
struct Buf<T = A, const N: usize = 2> {
    data: [u8; N],
    #[deku(count = "N")]
    tail: Vec<u8>,
    _s: PhantomData<T>,
}

impl_deku_read_write!(Buf);
impl_deku_read_write!(Buf<B, 3>);

#[test]
fn const_generics_and_defaults() {
    let (_, d) = Buf::<A, 2>::from_bytes((&[1, 2, 3, 4], 0)).unwrap();
    assert_eq!(d.data, [1, 2]);
    assert_eq!(d.tail, vec![3, 4]);

    let (_, b) = Buf::<B, 3>::from_bytes((&[1, 2, 3, 4, 5, 6], 0)).unwrap();
    assert_eq!(b.data, [1, 2, 3]);
    assert_eq!(b.tail, vec![4, 5, 6]);
    assert_eq!(b.to_bytes().unwrap(), [1, 2, 3, 4, 5, 6]);
}

// ---------------------------------------------------------------------------
// lifetimes
// ---------------------------------------------------------------------------

// deku 0.20's own derive only supports lifetime-carrying structs when it does
// not have to emit the container impls, i.e. with a `ctx`; same here.
// (deku's `Cow` impl needs a sized `T: Clone`, hence `Cow<Vec<u8>>`.)
#[allow(clippy::owned_cow)]
#[deku_generic]
#[derive(Debug, PartialEq)]
#[deku(ctx = "endian: Endian")]
struct Borrowed<'a, T> {
    #[deku(endian = "endian")]
    len: u16,
    #[deku(count = "len")]
    data: Cow<'a, Vec<u8>>,
    _s: PhantomData<T>,
}

impl_deku_read_write!(Borrowed<'a, A>);
impl_deku_write!(Borrowed<B>);
impl_deku_write!(Borrowed<'_, Buf>);

#[test]
fn lifetime_parameters() {
    let bytes = [0, 2, 7, 8];
    let mut cursor = std::io::Cursor::new(&bytes[..]);
    let mut reader = Reader::new(&mut cursor);
    let b = Borrowed::<A>::from_reader_with_ctx(&mut reader, Endian::Big).unwrap();
    assert_eq!(&*b.data, &[7, 8]);

    let owned = vec![7, 8];
    let borrowed = Borrowed::<B> {
        len: 2,
        data: Cow::Borrowed(&owned),
        _s: PhantomData,
    };
    let mut out = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut out);
    let mut writer = Writer::new(&mut cursor);
    borrowed.to_writer(&mut writer, Endian::Big).unwrap();
    writer.finalize().unwrap();
    assert_eq!(out, bytes);
}

// ---------------------------------------------------------------------------
// ctx and ctx_default
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, DekuRead, DekuWrite)]
#[deku(ctx = "endian: Endian, len: usize")]
struct PlainCtx {
    #[deku(endian = "endian")]
    id: u16,
    #[deku(count = "len")]
    body: Vec<u8>,
}

#[deku_generic]
#[derive(Debug, PartialEq)]
#[deku(ctx = "endian: Endian, len: usize")]
struct WithCtx<T> {
    #[deku(endian = "endian")]
    id: u16,
    #[deku(count = "len")]
    body: Vec<u8>,
    _s: PhantomData<T>,
}

impl_deku_read_write!(WithCtx<A>);

#[test]
fn context_struct() {
    let bytes = [0x01, 0x02, 0xaa];
    let mut cursor = std::io::Cursor::new(&bytes[..]);
    let mut reader = Reader::new(&mut cursor);
    let plain = PlainCtx::from_reader_with_ctx(&mut reader, (Endian::Big, 1)).unwrap();

    let mut cursor = std::io::Cursor::new(&bytes[..]);
    let mut reader = Reader::new(&mut cursor);
    let generic = WithCtx::<A>::from_reader_with_ctx(&mut reader, (Endian::Big, 1)).unwrap();
    assert_eq!(generic.id, plain.id);
    assert_eq!(generic.body, plain.body);

    let mut out = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut out);
    let mut writer = Writer::new(&mut cursor);
    generic.to_writer(&mut writer, (Endian::Big, 1)).unwrap();
    writer.finalize().unwrap();
    assert_eq!(out, bytes);
}

#[deku_generic]
#[derive(Debug, PartialEq)]
#[deku(ctx = "endian: Endian", ctx_default = "Endian::Little")]
struct WithDefaultCtx<T> {
    #[deku(endian = "endian")]
    id: u16,
    _s: PhantomData<T>,
}

impl_deku_read_write!(WithDefaultCtx<A>);

#[test]
fn context_default_gives_container_impls() {
    let (_, v) = WithDefaultCtx::<A>::from_bytes((&[0x01, 0x02], 0)).unwrap();
    assert_eq!(v.id, 0x0201);
    assert_eq!(v.to_bytes().unwrap(), [0x01, 0x02]);

    let mut cursor = std::io::Cursor::new(&[0x01u8, 0x02][..]);
    let mut reader = Reader::new(&mut cursor);
    let big = WithDefaultCtx::<A>::from_reader_with_ctx(&mut reader, Endian::Big).unwrap();
    assert_eq!(big.id, 0x0102);
}

// ---------------------------------------------------------------------------
// custom reader / writer expressions and field ctx
// ---------------------------------------------------------------------------

fn read_plus_one<R: std::io::Read + std::io::Seek>(
    reader: &mut Reader<R>,
) -> Result<u8, DekuError> {
    let v = u8::from_reader_with_ctx(reader, ())?;
    Ok(v + 1)
}

fn write_minus_one<W: std::io::Write + std::io::Seek>(
    v: &u8,
    writer: &mut Writer<W>,
) -> Result<(), DekuError> {
    (v - 1).to_writer(writer, ())
}

#[deku_generic]
#[derive(Debug, PartialEq)]
struct Custom<T> {
    #[deku(
        reader = "read_plus_one(deku::reader)",
        writer = "write_minus_one(value, deku::writer)"
    )]
    value: u8,
    #[deku(ctx = "*value as usize")]
    inner: Inner,
    _s: PhantomData<T>,
}

#[derive(Debug, PartialEq, DekuRead, DekuWrite)]
#[deku(ctx = "n: usize")]
struct Inner {
    #[deku(count = "n")]
    bytes: Vec<u8>,
}

impl_deku_read_write!(Custom<A>);

#[test]
fn custom_reader_writer_and_field_ctx() {
    let bytes = [1, 0xaa, 0xbb];
    let (_, c) = Custom::<A>::from_bytes((&bytes, 0)).unwrap();
    assert_eq!(c.value, 2);
    assert_eq!(c.inner.bytes, vec![0xaa, 0xbb]);
    assert_eq!(c.to_bytes().unwrap(), bytes);
}

// ---------------------------------------------------------------------------
// attribute form and cross-module paths
// ---------------------------------------------------------------------------

#[deku_generic(read(Inline<A>), write(Inline<A>, Inline<B>))]
#[derive(Debug, PartialEq)]
struct Inline<T> {
    x: u8,
    _s: PhantomData<T>,
}

#[test]
fn attribute_form() {
    let (_, v) = Inline::<A>::from_bytes((&[5], 0)).unwrap();
    assert_eq!(v.x, 5);
    assert_eq!(v.to_bytes().unwrap(), [5]);
    let b = Inline::<B> {
        x: 6,
        _s: PhantomData,
    };
    assert_eq!(b.to_bytes().unwrap(), [6]);
}

mod proto {
    use std::marker::PhantomData;

    use deku_generic::deku_generic;

    #[deku_generic(read_write(Remote<super::A>))]
    #[derive(Debug, PartialEq)]
    pub struct Remote<T> {
        pub x: u8,
        pub _s: PhantomData<T>,
    }
}

impl_deku_write!(proto::Remote<B>);

#[test]
fn cross_module_path() {
    let (_, v) = proto::Remote::<A>::from_bytes((&[5], 0)).unwrap();
    assert_eq!(v.x, 5);
    let b = proto::Remote::<B> {
        x: 6,
        _s: PhantomData,
    };
    assert_eq!(b.to_bytes().unwrap(), [6]);
}
