use core::marker::PhantomData;
use deku_generic::{deku_generic, impl_deku_write};

pub struct Raw;

#[deku_generic]
pub struct Foo<T> {
    #[deku(update = 3)]
    a: u8,
    _state: PhantomData<T>,
}

impl_deku_write!(Foo<Raw>);

fn main() {}
