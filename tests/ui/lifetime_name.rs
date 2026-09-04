use core::marker::PhantomData;
use deku_generic::{deku_generic, impl_deku_read};

pub struct Raw;

#[deku_generic]
#[deku(ctx = "n: usize")]
pub struct Foo<'a, T> {
    #[deku(count = "n")]
    data: Vec<u8>,
    _state: PhantomData<&'a T>,
}

impl_deku_read!(Foo<'static, Raw>);

fn main() {}
