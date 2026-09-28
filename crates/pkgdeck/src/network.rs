//! Qt owns HTTP, TLS validation, and the bounded disk cache used by QML images.
use pkgdeck_core::process::Cancellation;

#[cxx::bridge(namespace = "pkgdeck")]
pub mod ffi {
    // The GUI entry point (native/main.cpp) calls the network and opening
    // setup directly; Rust only needs the metadata download.
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        include!("pkgdeck/native/providers.h");
        #[namespace = ""]
        type QString = cxx_qt_lib::QString;
        fn fetch_metadata(url: &QString, cancel: &LookupCancellation) -> QString;
    }
    extern "Rust" {
        type LookupCancellation;
        fn cancelled(&self) -> bool;
    }
}
/// Downloads a provider's JSON over HTTPS through Qt; any other scheme, a
/// failure, or a cancellation returns an empty string.
pub fn fetch(url: &str, cancel: &Cancellation) -> String {
    ffi::fetch_metadata(&url.into(), &LookupCancellation(cancel.clone())).to_string()
}
pub struct LookupCancellation(pub Cancellation);
impl LookupCancellation {
    fn cancelled(&self) -> bool {
        self.0.requested()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_cancellation_reports_requested_state() {
        let cancel = Cancellation::default();
        let lookup = LookupCancellation(cancel.clone());
        assert!(!lookup.cancelled());
        cancel.cancel();
        assert!(lookup.cancelled());
    }
    #[test]
    fn fetch_refuses_plain_http_and_cancelled_lookups_before_connecting() {
        // Neither address is ever contacted: the scheme and the cancellation
        // are checked before Qt creates a request.
        let cancel = Cancellation::default();
        assert_eq!(fetch("http://127.0.0.1:9/app.json", &cancel), "");
        assert_eq!(fetch("file:///etc/hostname", &cancel), "");
        cancel.cancel();
        assert_eq!(fetch("https://127.0.0.1:9/app.json", &cancel), "");
    }
}
