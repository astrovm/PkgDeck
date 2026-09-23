use pkgdeck as _;

#[allow(unsafe_code)]
unsafe extern "C" {
    fn pkgdeck_run_gui(
        argc: i32,
        argv: *mut *mut std::ffi::c_char,
        version: *const std::ffi::c_char,
    ) -> i32;
}

#[allow(unsafe_code)]
fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("pkgdeck {}", pkgdeck_core::VERSION);
        return;
    }
    let mut args: Vec<Vec<u8>> = std::env::args_os()
        .map(|arg| {
            use std::os::unix::ffi::OsStrExt;
            std::ffi::CString::new(arg.as_os_str().as_bytes())
                .expect("argument contains NUL")
                .into_bytes_with_nul()
        })
        .collect();
    let mut argv: Vec<*mut std::ffi::c_char> = args
        .iter_mut()
        .map(|arg| arg.as_mut_ptr().cast())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();
    let version = std::ffi::CString::new(pkgdeck_core::VERSION).unwrap();
    // QApplication may reorder or edit argv; the mutable byte buffers stay
    // alive until the Qt event loop exits.
    let exit_code =
        unsafe { pkgdeck_run_gui(args.len() as i32, argv.as_mut_ptr(), version.as_ptr()) };
    std::process::exit(exit_code);
}
