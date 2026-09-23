//! Shared, Qt-independent foundations for PkgDeck frontends.

pub const APP_ID: &str = "io.github.astrovm.PkgDeck";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const FOUNDATION_MESSAGE: &str =
    "Package management is not available yet. This build contains the application foundation.";

pub mod host;
pub mod inspection;
pub mod process;

pub mod activity;
pub mod background;
pub mod engine;
pub mod package;

pub mod backends;

pub mod repositories;
