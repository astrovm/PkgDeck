//! Dev-dependency of `pkgdeck-core`.
//!
//! Depending on `pkgdeck-core` with the `test-support` feature makes Cargo
//! unify that feature into the package under test, so `crates/pkgdeck-core/tests`
//! can call `pkgdeck_core::test_support`. The library has no tests of its own.
