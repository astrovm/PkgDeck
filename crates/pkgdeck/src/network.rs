//! Qt owns HTTP, TLS validation, and the bounded disk cache used by QML images.
use pkgdeck_core::process::Cancellation;

#[cxx::bridge(namespace = "pkgdeck")]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qqmlapplicationengine.h");
        include!("cxx-qt-lib/qstring.h");
        include!("pkgdeck/native/network.h");
        include!("pkgdeck/native/providers.h");
        #[namespace = ""]
        type QQmlApplicationEngine = cxx_qt_lib::QQmlApplicationEngine;
        #[namespace = ""]
        type QString = cxx_qt_lib::QString;
        fn configure_network(engine: Pin<&mut QQmlApplicationEngine>);
        fn fetch_metadata(url: &QString, cancel: &LookupCancellation) -> QString;
    }
    extern "Rust" {
        type LookupCancellation;
        fn cancelled(&self) -> bool;
    }
}
pub struct LookupCancellation(pub Cancellation);
impl LookupCancellation {
    fn cancelled(&self) -> bool {
        self.0.requested()
    }
}
