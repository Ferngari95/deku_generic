use deku_generic::deku_generic;

#[deku_generic]
pub enum Packet<T> {
    A(T),
}

fn main() {}
