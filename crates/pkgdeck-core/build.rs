use std::{env, path::Path, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=../../native/apt-query.cpp");
    println!("cargo:rerun-if-changed=../../scripts/build-apt.sh");
    println!("cargo:rerun-if-changed=/usr/include/apt-pkg/cachefile.h");
    for variable in ["CXX", "CPPFLAGS", "LDFLAGS", "PKGDECK_APT_LIB"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    // Keep non-APT platforms and cross builds independent of libapt-pkg.
    // Distribution bundles provide their own adjacent helper.
    if env::var("HOST") != env::var("TARGET")
        || !Path::new("/usr/include/apt-pkg/cachefile.h").is_file()
    {
        return;
    }
    let output = env::var("OUT_DIR").expect("Cargo supplies OUT_DIR");
    let status = Command::new("bash")
        .args(["../../scripts/build-apt.sh", &output])
        .status()
        .expect("run APT helper compiler");
    assert!(status.success(), "APT helper compilation failed");
    println!("cargo:rustc-env=PKGDECK_BUILT_APT_QUERY={output}/pkgdeck-apt-query");
}
