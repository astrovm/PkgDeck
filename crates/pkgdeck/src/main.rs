use cxx_qt::casting::Upcast;
use cxx_qt_lib::{QCoreApplication, QGuiApplication, QQmlApplicationEngine};
use pkgdeck as _;

fn main() {
    if std::env::args().any(|arg| arg == "--version") {
        println!("pkgdeck {}", pkgdeck_core::VERSION);
        return;
    }
    let mut app = QGuiApplication::new();
    {
        let mut core = Upcast::<QCoreApplication>::upcast_pin(app.as_mut().unwrap());
        core.as_mut().set_application_name(&"PkgDeck".into());
        core.as_mut()
            .set_application_version(&pkgdeck_core::VERSION.into());
        core.as_mut().set_organization_name(&"astrovm".into());
        core.set_organization_domain(&"github.com/astrovm".into());
    }
    QGuiApplication::set_desktop_file_name(&pkgdeck_core::APP_ID.into());
    let mut engine = QQmlApplicationEngine::new();
    pkgdeck::network::ffi::configure_network(engine.as_mut().unwrap());
    let _failure = engine.as_mut().unwrap().on_object_creation_failed(|_, _| {
        std::process::exit(1);
    });
    engine
        .as_mut()
        .unwrap()
        .load(&"qrc:/qt/qml/io/github/astrovm/PkgDeck/qml/Main.qml".into());
    app.as_mut().unwrap().exec();
}
