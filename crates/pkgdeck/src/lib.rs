//! PkgDeck's window, drawn with egui. [`app`] holds the state and talks to
//! the controller in `pkgdeck-app`; [`ui`] draws it; [`platform`] is the
//! tray, notifications and Dock badge; [`opening`] keeps one PkgDeck per
//! person; [`settings`] remembers choices; [`media`] fetches images.

pub mod app;
pub mod media;
pub mod model;
pub mod opening;
pub mod platform;
pub mod settings;
pub mod ui;

pub use app::App;
pub use ui::icons::NAMES as ICON_NAMES;
pub use ui::theme;
pub use ui::window::{setup, wake, Window};
