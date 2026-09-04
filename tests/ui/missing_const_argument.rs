use deku_generic::{deku_generic, impl_deku_read};

#[deku_generic]
pub struct Buf<const N: usize> {
    data: [u8; N],
}

impl_deku_read!(Buf);

fn main() {}
