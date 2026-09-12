use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine};

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("pkgdeck {}", pkgdeck_core::VERSION);
        return;
    }
    let mut app = QGuiApplication::new();
    QGuiApplication::set_desktop_file_name(&pkgdeck_core::APP_ID.into());
    let mut engine = QQmlApplicationEngine::new();
    let _failure = engine.as_mut().unwrap().on_object_creation_failed(|_, _| {
        std::process::exit(1);
    });
    engine
        .as_mut()
        .unwrap()
        .load(&"qrc:/qt/qml/io/github/astrovm/PkgDeck/qml/Main.qml".into());
    app.as_mut().unwrap().exec();
}
