//! The motivating example: a typestate struct that is readable in one state
//! and writable in two.

use std::marker::PhantomData;

use deku::prelude::*;
use deku_generic::{deku_generic, impl_deku_read, impl_deku_write};

#[derive(Debug, PartialEq, DekuRead, DekuWrite)]
pub struct Unvalidated;

#[derive(Debug, PartialEq, DekuWrite)]
pub struct Validated;

#[deku_generic]
#[derive(Debug, PartialEq)]
pub struct Foo<T> {
    _state: PhantomData<T>,
}

impl_deku_read!(Foo<Unvalidated>);
impl_deku_write!(Foo<Unvalidated>);
impl_deku_write!(Foo<Validated>);

#[test]
fn unit_like_typestate_round_trips() {
    let (rest, foo) = Foo::<Unvalidated>::from_bytes((&[], 0)).unwrap();
    assert_eq!(rest, (&[][..], 0));
    assert_eq!(
        foo,
        Foo {
            _state: PhantomData
        }
    );
    assert_eq!(foo.to_bytes().unwrap(), Vec::<u8>::new());

    let validated = Foo::<Validated> {
        _state: PhantomData,
    };
    assert_eq!(validated.to_bytes().unwrap(), Vec::<u8>::new());
    let via_try: Vec<u8> = validated.try_into().unwrap();
    assert!(via_try.is_empty());
}

/// Compile-time check that only the requested impls exist.
#[allow(dead_code)]
fn trait_surface() {
    fn reader<'a, T: DekuReader<'a> + DekuContainerRead<'a> + TryFrom<&'a [u8]>>() {}
    fn writer<T: DekuWriter + DekuContainerWrite + DekuUpdate>() {}
    fn writer_owned<T>()
    where
        Vec<u8>: TryFrom<T>,
    {
    }

    reader::<Foo<Unvalidated>>();
    writer::<Foo<Unvalidated>>();
    writer::<Foo<Validated>>();
    writer_owned::<Foo<Validated>>();
}

/// Negative check through autoref specialisation: `Foo<Validated>` must not
/// implement `DekuReader`.
#[test]
#[allow(clippy::needless_borrow)]
fn validated_is_not_readable() {
    struct Probe<T>(PhantomData<T>);

    // Fallback: needs an autoref step, so it loses when `Readable` applies.
    trait NotReadable {
        fn is_readable(&self) -> bool {
            false
        }
    }
    impl<T> NotReadable for &Probe<T> {}

    trait Readable {
        fn is_readable(&self) -> bool {
            true
        }
    }
    impl<T: for<'a> DekuReader<'a>> Readable for Probe<T> {}

    assert!((&Probe::<Foo<Unvalidated>>(PhantomData)).is_readable());
    assert!(!(&Probe::<Foo<Validated>>(PhantomData)).is_readable());
}
