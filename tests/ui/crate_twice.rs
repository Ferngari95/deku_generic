use core::marker::PhantomData;
use deku_generic::deku_generic;

#[deku_generic(crate = "deku_generic", crate = "deku_generic")]
pub struct Foo<T> {
    a: u8,
    _state: PhantomData<T>,
}

fn main() {}
