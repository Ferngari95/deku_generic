use core::marker::PhantomData;
use deku_generic::deku_generic;

#[deku_generic(crate = 4)]
pub struct Foo<T> {
    a: u8,
    _state: PhantomData<T>,
}

fn main() {}
