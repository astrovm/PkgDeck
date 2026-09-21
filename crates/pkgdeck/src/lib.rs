// CXX-Qt generates the FFI glue; the controller implementation uses safe Rust.
#[allow(unsafe_code)]
mod controller;

mod metadata;

#[allow(unsafe_code)]
pub mod network;
