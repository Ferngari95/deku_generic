use core::marker::PhantomData;
use deku_generic::deku_generic;

pub struct Raw;

#[deku_generic(parse(Foo<Raw>))]
pub struct Foo<T> {
    a: u8,
    _state: PhantomData<T>,
}

fn main() {}
