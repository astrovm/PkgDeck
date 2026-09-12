use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("io.github.astrovm.PkgDeck").qml_file("qml/Main.qml"),
    )
    .build();
}
