import QtQuick
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami

Kirigami.ApplicationWindow {
    id: root
    width: 960
    height: 640
    visible: true
    title: "PkgDeck"

    pageStack.initialPage: Kirigami.Page {
        title: "Welcome"
        Controls.Label {
            anchors.centerIn: parent
            width: Math.min(parent.width - 40, 520)
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            text: "Package management is not available yet. This build contains the application foundation."
        }
    }

    // Load the actual window and its imports before ending a CI smoke test.
    Timer {
        interval: 100
        running: Qt.application.arguments.indexOf("--smoke-test") !== -1
        onTriggered: {
            console.info("PKGDECK_GUI_READY")
            Qt.quit()
        }
    }
}
