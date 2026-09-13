import QtQuick
import io.github.astrovm.PkgDeck

Browser {
    backend: PackageController {}
    Timer {
        interval: 150
        running: Qt.application.arguments.indexOf("--smoke-test") !== -1
        onTriggered: {
            console.info("PKGDECK_GUI_READY");
            Qt.quit();
        }
    }
}
