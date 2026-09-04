use core::marker::PhantomData;
use deku_generic::{deku_generic, impl_deku_read};

pub struct Raw;

#[deku_generic]
pub struct Foo<T> {
    #[deku(temp)]
    len: u8,
    #[deku(count = "len")]
    data: Vec<u8>,
    _state: PhantomData<T>,
}

impl_deku_read!(Foo<Raw>);

fn main() {}
