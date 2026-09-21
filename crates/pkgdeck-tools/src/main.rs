use pkgdeck_tools::*;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (kind, args) = args
        .split_first()
        .expect("Usage: cargo xtask gui|gui-failure|gui-lifecycle|qml|gui-write COMMAND...");
    match kind.as_str() {
        "gui" => gui(args, false),
        "gui-failure" => gui(args, true),
        "gui-lifecycle" => gui_lifecycle(args),
        "qml" => qml(),
        "apt-lock-probe" => apt_lock_probe(),
        "gui-write" => {
            let mut g = Desktop::new();
            g.launch(&args[2..]);
            g.write(&args[0], &args[1]);
            g.close();
        }
        _ => panic!("unknown task {kind}"),
    }
}
