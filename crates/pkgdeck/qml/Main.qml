import QtQuick
import Qt.labs.platform as Platform
import io.github.astrovm.PkgDeck

Browser {
    id: browser
    backend: PackageController {}
    trayAvailable: tray.available
    Platform.SystemTrayIcon {
        id: tray
        visible: browser.backgroundMode && available
        icon.source: browser.logoIconSource
        tooltip: "PkgDeck"
        menu: Platform.Menu {
            Platform.MenuItem { text: "Open"; onTriggered: { browser.show(); browser.raise(); browser.requestActivate(); } }
            Platform.MenuItem { text: "Check now"; onTriggered: browser.checkUpdates(true) }
            Platform.MenuItem { text: "Quit"; onTriggered: { browser.forceQuit = true; Qt.quit(); } }
        }
        onMessageClicked: { browser.show(); browser.raise(); browser.requestActivate(); browser.openView("Updates"); }
    }
    Connections {
        target: browser.backend
        function onBackgroundStateChanged() {
            if (browser.backgroundState.notify && tray.visible && tray.supportsMessages)
                tray.showMessage("PkgDeck updates", browser.backgroundState.available + " updates available", Platform.SystemTrayIcon.Information, 8000);
        }
    }
    Timer {
        interval: 150
        running: Qt.application.arguments.indexOf("--smoke-test") !== -1
        onTriggered: {
            console.info("PKGDECK_GUI_READY");
            Qt.quit();
        }
    }
}
