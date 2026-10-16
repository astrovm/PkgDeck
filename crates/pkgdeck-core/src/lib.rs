//! Shared, Qt-independent foundations for PkgDeck frontends.

pub const APP_ID: &str = "io.github.astrovm.PkgDeck";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const FOUNDATION_MESSAGE: &str =
    "Package management is not available yet. This build contains the application foundation.";

pub mod batch;
pub mod cache;
pub mod host;
pub mod inspection;
#[cfg(target_os = "linux")]
pub mod packagekit;
#[cfg(target_os = "linux")]
mod portal;
pub mod private_index;
pub mod process;
mod spawn;

pub mod activity;
pub mod artifact;
pub mod background;
pub mod disk;
pub mod engine;
pub mod failed_updates;
pub mod flatpak_ref;
pub mod hold;
pub mod local_deb;
pub mod manifest;
pub mod needs_password;
pub mod package;
pub mod relaunch;
pub mod unattended;

pub mod backends;

pub mod repositories;
pub mod repository_input;
