use core::marker::PhantomData;
use deku_generic::deku_generic;

pub struct Raw;

#[deku_generic(read(Bar<Raw>))]
pub struct Foo<T> {
    a: u8,
    _state: PhantomData<T>,
}

fn main() {}
