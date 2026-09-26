import QtQuick
import Qt.labs.platform as Platform
import io.github.astrovm.PkgDeck

Browser {
    id: browser
    backend: PackageController {}
    trayAvailable: tray.available
    notificationAvailable: tray.available && tray.supportsMessages
    onTestNotificationRequested: tray.showMessage("PkgDeck test", "Desktop notifications are working.", Platform.SystemTrayIcon.Information, 8000)
    Platform.SystemTrayIcon {
        id: tray
        visible: browser.backgroundMode && available
        icon.source: browser.logoIconSource
        tooltip: "PkgDeck"
        onActivated: function(reason) {
            if (reason === Platform.SystemTrayIcon.Trigger)
                browser.toggleFromTray();
        }
        menu: Platform.Menu {
            Platform.MenuItem { text: "Open"; onTriggered: browser.showFromTray() }
            Platform.MenuItem { text: "Check now"; onTriggered: browser.checkUpdates(true) }
            Platform.MenuItem { text: "Quit"; onTriggered: { browser.forceQuit = true; Qt.quit(); } }
        }
        onMessageClicked: { browser.showFromTray(); browser.openView("Updates"); }
    }
    Connections {
        target: browser
        function onBackgroundStateChanged() {
            if (browser.backgroundState.notify && tray.visible && tray.supportsMessages) {
                const count = browser.backgroundState.available;
                tray.showMessage("PkgDeck updates", count + (count === 1 ? " update" : " updates") + " available", Platform.SystemTrayIcon.Information, 8000);
                browser.backend.acknowledgeNotification();
            }
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
