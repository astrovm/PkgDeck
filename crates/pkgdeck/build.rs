use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("io.github.astrovm.PkgDeck")
            .qml_file("qml/Main.qml")
            .qml_file("qml/Browser.qml")
            .qml_file("qml/SearchPane.qml")
            .qml_file("qml/UpdatesActions.qml")
            .qml_file("qml/PackageDetails.qml")
            .qml_file("qml/ActivityPane.qml")
            .qml_file("qml/ActionProgress.qml")
            .qml_file("qml/DeckIcon.qml")
            .qml_file("qml/DeckScrollBar.qml")
            .qml_file("qml/DeckScrollView.qml")
            .qml_file("qml/ClearFieldButton.qml")
            .qml_file("qml/RowProgress.qml")
            .qml_file("qml/Toast.qml"),
    )
    .qrc("resources.qrc")
    .file("src/controller.rs")
    .file("src/network.rs")
    .qt_module("Network")
    .qt_module("Widgets")
    .cpp_files([
        "native/main.cpp",
        "native/network.cpp",
        "native/providers.cpp",
        "native/controller.cpp",
        "native/opening.cpp",
    ])
    .build();
}
