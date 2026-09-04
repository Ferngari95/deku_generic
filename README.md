# deku_generic

Implements [deku](https://docs.rs/deku)'s `DekuReader`, `DekuWriter` and the
container traits for particular instantiations of a generic struct, rather
than for every `T`. Not on crates.io yet, so for now:

```toml
[dependencies]
deku_generic = { git = "https://github.com/Ferngari95/deku_generic" }
```

`#[derive(DekuRead)]` on `struct Foo<T>` gives you `impl<T> DekuReader for Foo<T>`.
When `T` is a typestate marker that is usually the wrong thing: a
`Foo<Validated>` should not be parseable straight off the wire, only a
`Foo<Unvalidated>` should, while both of them may need to be written back out.
With this crate you say which ones you want:

```rust
use core::marker::PhantomData;
use deku::prelude::*;
use deku_generic::{deku_generic, impl_deku_read, impl_deku_write};

pub struct Unvalidated;
pub struct Validated;

#[deku_generic]                     // in place of #[derive(DekuRead, DekuWrite)]
#[derive(Debug, PartialEq)]
pub struct Foo<T> {
    #[deku(bits = 4)]
    kind: u8,
    #[deku(bits = 4)]
    len: u8,
    #[deku(count = "len")]
    data: Vec<u8>,
    _state: PhantomData<T>,
}

impl_deku_read!(Foo<Unvalidated>);
impl_deku_write!(Foo<Unvalidated>);
impl_deku_write!(Foo<Validated>);

let (_, foo) = Foo::<Unvalidated>::from_bytes((&[0x12, 0xaa, 0xbb], 0)).unwrap();
assert_eq!(foo.data, [0xaa, 0xbb]);
// Foo::<Validated>::from_bytes(..) is a compile error.
```

`impl_deku_read!` gives you `DekuReader`, `DekuContainerRead` and
`TryFrom<&[u8]>`. `impl_deku_write!` gives you `DekuWriter`,
`DekuContainerWrite`, `DekuUpdate` and `TryFrom<Foo<..>> for Vec<u8>`, the
same set deku's derives would produce. `impl_deku_read_write!` does both.
The container/`TryFrom` impls are only generated when deku would generate
them, i.e. when there is no `ctx` or there is a `ctx_default`.

If you'd rather keep the list on the struct:

```rust,ignore
#[deku_generic(read(Foo<Unvalidated>), write(Foo<Unvalidated>, Foo<Validated>))]
pub struct Foo<T> { .. }
```

(`read_write(..)` is accepted too.) Fields of type `PhantomData<..>` that
carry no `#[deku]` attribute are treated as `#[deku(skip)]`, since deku has
no impl for `PhantomData` and you'd have to write that yourself otherwise.

The generated code refers to this crate as `::deku_generic`. If it is only
reachable under another name, add `crate = "path::to::deku_generic"` to the
attribute.

## no_std

The crate is `#![no_std]`. The generated code needs `alloc` for `Vec<u8>`
(`to_bytes`, `TryFrom<Foo> for Vec<u8>`), so turn off deku's default features
and enable `alloc`; add `bits` if you use `bits = ..`:

```toml
[dependencies]
deku = { version = "0.20", default-features = false, features = ["alloc"] }
deku_generic = { git = "https://github.com/Ferngari95/deku_generic" }
```

There is a `no_std_check` crate in the repository that CI builds for
`thumbv6m-none-eabi`.

## Versions

deku_generic 0.1 depends on deku 0.20 and the generated impls use that copy,
so Cargo keeps the two in step: a deku of another major version in your own
`Cargo.toml` shows up as a version mismatch, not as a missing trait.
Edition 2024, MSRV 1.85.

## How it works

The macros don't reimplement deku's attribute handling. For each requested
instantiation they emit an anonymous `const _: () = { .. }` block containing
a type alias per generic parameter (`type T = Unvalidated;`,
`const N: usize = 4;`) and a hidden, non-generic copy of the struct with the
same fields and the same `#[deku(..)]` attributes. deku's own derive runs on
that copy. The impls for `Foo<Unvalidated>` delegate to it: on the read side
the copy's fields are moved into `Foo`, on the write side a copy whose fields
are `&`-references into `self` is built and written, which works because
deku implements `DekuWriter` for `&T`. Those impls name deku through a
hidden re-export from this crate's own deku dependency, so they are always
compiled against the deku version this crate was written for. The derive on
the copy is deku's, and it locates deku by itself.

One wrinkle on the write side: deku hands attribute expressions (`cond`,
`assert`, `assert_eq`, `ctx`, `writer`, ..) the sibling fields as `&T`
locals. On the reference copy those would be `&&T`, so any expression that
mentions a field gets wrapped in a block that rebinds it first. Expressions
written for a plain `#[derive(DekuWrite)]` therefore work unchanged. This
was tested against plain derives for `endian`, `bits`, `bytes`, `count`,
`cond`, `assert`, `assert_eq`, `update`, `pad_*`, `reader`/`writer`, field
and struct `ctx`, `ctx_default`, tuple structs, where clauses, const generics
and type parameter defaults.

## Limitations

- Structs only. Enums would need the same trick per variant; not done.
- `#[deku(temp)]` is rejected (it changes the field list).
- `impl_deku_*!` has to be invoked in the module that defines the struct,
  or the struct has to be named by path (`impl_deku_write!(crate::proto::Foo<X>)`)
  with its fields visible from the call site. The attribute form doesn't
  have this problem.
- Field types can't mention `Self`, and the concrete arguments can't mention
  lifetimes other than `'static`.
- Structs with lifetime parameters work as far as deku's derive supports
  them, which in 0.20 means they need a `ctx`.
- `deku` still has to be a dependency of your crate, under that name: deku's
  own derive runs on the hidden copy, and its output names `deku`.

## Layout

- `deku_generic`: the crate to depend on. Re-exports the macros, has the
  docs and tests.
- `deku_generic_macros`: the proc macros.
- `no_std_check`: not published, bare-metal build check.

`cargo test` at the root runs everything, including this README's code
blocks. The compile-fail cases in `tests/ui` compare against `.stderr`
snapshots; after changing a diagnostic, `TRYBUILD=overwrite cargo test
--test ui` rewrites them.

## Releasing

Both crates carry `publish = false` until the first release. To release:
remove those two lines, bump `workspace.package.version` and the `=x.y.z`
pin in the root `Cargo.toml`, update `CHANGELOG.md`, put the crates.io
badges back, then `cargo publish -p deku_generic_macros` followed by
`cargo publish -p deku_generic`. The macros crate has to be on crates.io
before the facade can even be packaged, so `cargo publish --dry-run` only
covers the first of the two; run it for the facade after the macros are up.
From the second release on, `cargo semver-checks` against the previous
version is worth a CI job.

## License

MIT or Apache-2.0, at your option. See `LICENSE-MIT` and `LICENSE-APACHE`.
