use pkgdeck as _;

#[allow(unsafe_code)]
unsafe extern "C" {
    fn pkgdeck_run_gui(
        argc: i32,
        argv: *mut *mut std::ffi::c_char,
        version: *const std::ffi::c_char,
    ) -> i32;
}

/// Finder and the Dock start apps with only the system PATH, so add Homebrew's
/// directories for the managers it installs, and pick the Basic Qt Quick
/// Controls style, which PkgDeck's themed controls are written against.
#[cfg(target_os = "macos")]
fn prepare_macos_environment() {
    if std::env::var_os("QT_QUICK_CONTROLS_STYLE").is_none() {
        std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Basic");
    }
    let current =
        std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    // Finder starts apps with a minimal PATH; add Homebrew's and MacPorts'
    // folders (Homebrew's end up first).
    for dir in [
        "/opt/local/sbin",
        "/opt/local/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
        "/opt/homebrew/sbin",
        "/opt/homebrew/bin",
    ] {
        if !dirs.iter().any(|existing| existing.as_os_str() == dir) {
            dirs.insert(0, dir.into());
        }
    }
    if let Ok(path) = std::env::join_paths(dirs) {
        std::env::set_var("PATH", path);
    }
}

/// The Homebrew cask links the bundled binary into Homebrew's bin directory.
/// Qt finds the bundle's plugins and QML modules from the executable path,
/// which a symlink hides, so run the binary from inside the bundle instead.
#[cfg(target_os = "macos")]
fn exec_from_bundle() {
    use std::os::unix::process::CommandExt;
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Ok(real) = executable.canonicalize() else {
        return;
    };
    let in_bundle = real
        .parent()
        .is_some_and(|dir| dir.ends_with("Contents/MacOS"));
    if !in_bundle || real == executable {
        return;
    }
    let error = std::process::Command::new(&real)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("pkgdeck: cannot run {}: {error}", real.display());
    std::process::exit(1);
}

#[allow(unsafe_code)]
fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("pkgdeck {}", pkgdeck_core::VERSION);
        return;
    }
    #[cfg(target_os = "macos")]
    {
        exec_from_bundle();
        prepare_macos_environment();
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
