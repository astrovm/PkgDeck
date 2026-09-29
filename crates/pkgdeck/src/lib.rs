// CXX-Qt generates the FFI glue; the controller implementation uses safe Rust.
#[allow(unsafe_code)]
mod controller;

mod metadata;

#[allow(unsafe_code)]
pub mod network;

// native/macos.mm posts notifications and sets the Dock badge.
#[cfg(target_os = "macos")]
#[link(name = "AppKit", kind = "framework")]
extern "C" {}
#[cfg(target_os = "macos")]
#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}
