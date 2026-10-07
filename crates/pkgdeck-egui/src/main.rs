//! `pkgdeck-egui`: PkgDeck's Installed page without Qt. `--smoke-test`
//! draws one frame, says so, and exits.

fn main() -> eframe::Result {
    let smoke_test = std::env::args().any(|arg| arg == "--smoke-test");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("PkgDeck")
            .with_app_id("io.github.astrovm.PkgDeck")
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([480.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PkgDeck",
        options,
        Box::new(move |cc| {
            Ok(Box::new(pkgdeck_egui::Window::new(
                &cc.egui_ctx,
                pkgdeck_core::backends::native_engine,
                smoke_test,
            )))
        }),
    )
}
