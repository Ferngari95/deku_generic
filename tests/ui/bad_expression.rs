use core::marker::PhantomData;
use deku_generic::deku_generic;

#[deku_generic]
pub struct Foo<T> {
    #[deku(cond = "a == (1")]
    a: u8,
    _state: PhantomData<T>,
}

fn main() {}
