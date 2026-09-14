use pkgdeck_tools::*;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (kind,args)=args.split_first().expect("Usage: cargo xtask terminal|terminal-interactions|gui|gui-failure|gui-lifecycle|qml|tui-write|gui-write COMMAND...");
    match kind.as_str() {
        "terminal" => terminal(args),
        "terminal-interactions" => terminal_interactions(args),
        "gui" => gui(args, false),
        "gui-failure" => gui(args, true),
        "gui-lifecycle" => gui_lifecycle(args),
        "qml" => qml(),
        "apt-lock-probe" => apt_lock_probe(),
        "tui-write" => tui_write(&args[2..], &args[0], &args[1]),
        "gui-write" => {
            let mut g = Desktop::new();
            g.launch(&args[2..]);
            g.write(&args[0], &args[1]);
            g.close();
        }
        _ => panic!("unknown task {kind}"),
    }
}
