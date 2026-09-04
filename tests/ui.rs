//! Compile-fail cases for the diagnostics the macros produce themselves.
//!
//! Errors that come from rustc rather than from `deku_generic` (a missing
//! `from_bytes` on a state that was not requested, say) are covered by the
//! doctests instead: their wording changes between toolchains, and this
//! suite runs on stable and on the MSRV.

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
