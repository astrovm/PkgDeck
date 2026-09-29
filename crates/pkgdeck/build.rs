use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    let mut builder = CxxQtBuilder::new_qml_module(
        QmlModule::new("io.github.astrovm.PkgDeck")
            .qml_file("qml/Main.qml")
            .qml_file("qml/Browser.qml")
            .qml_file("qml/ActionButton.qml")
            .qml_file("qml/ActionProgress.qml")
            .qml_file("qml/ActivityPane.qml")
            .qml_file("qml/ClearFieldButton.qml")
            .qml_file("qml/DeckIcon.qml")
            .qml_file("qml/DeckScrollBar.qml")
            .qml_file("qml/DeckScrollView.qml")
            .qml_file("qml/PackageDetails.qml")
            .qml_file("qml/RowProgress.qml")
            .qml_file("qml/SearchPane.qml")
            .qml_file("qml/SettingCheckBox.qml")
            .qml_file("qml/SettingsCard.qml")
            .qml_file("qml/SettingsPage.qml")
            .qml_file("qml/ThemedComboBox.qml")
            .qml_file("qml/ThemedDialog.qml")
            .qml_file("qml/ThemedTextField.qml")
            .qml_file("qml/TickBox.qml")
            .qml_file("qml/Toast.qml")
            .qml_file("qml/UpdatesActions.qml"),
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
    ]);
    // Native notifications and the Dock badge for the Mac app.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        builder = builder.cpp_files(["native/macos.h", "native/macos.mm"]);
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=UserNotifications");
    }
    builder.build();
}
