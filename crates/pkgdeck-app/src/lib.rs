//! What PkgDeck's window does, without a toolkit: the controller that loads
//! sections, opens packages, reviews and runs changes on worker threads,
//! and the app metadata it shows. A front end draws its properties and
//! calls its methods; `qt` keeps the interface the Qt app was written to.
pub mod controller;
pub mod metadata;
mod network;
pub mod wake;
pub mod qt;
