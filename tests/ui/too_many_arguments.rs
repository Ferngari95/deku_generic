use core::marker::PhantomData;
use deku_generic::{deku_generic, impl_deku_read};

pub struct Raw;

#[deku_generic]
pub struct Foo<T> {
    a: u8,
    _state: PhantomData<T>,
}

impl_deku_read!(Foo<Raw, u8>);

fn main() {}
