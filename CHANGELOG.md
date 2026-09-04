# Changelog

## 0.1.0 (2026-09-03)

First release.

- `#[deku_generic]` attribute for generic structs. Records the `#[deku(...)]`
  attributes and otherwise leaves the struct alone.
- `impl_deku_read!`, `impl_deku_write!`, `impl_deku_read_write!`, and the
  `read(..)` / `write(..)` / `read_write(..)` keys on the attribute.
- Type parameters (defaults honoured), const parameters, lifetime parameters,
  tuple structs, where clauses. All deku attributes except `temp`.
- `PhantomData` fields without deku attributes are skipped.
- `crate = ".."` on the attribute, for when `deku_generic` is reachable under
  another name.
- Depends on deku 0.20 and re-exports it for the generated code, so the
  impls always match the deku version this crate was built against. Your
  crate still needs `deku` under that name for deku's own derive.
- A rejected struct is still emitted, so one mistake gives one error.
- Works under `#![no_std]` with `alloc`. Edition 2024, MSRV 1.85.
