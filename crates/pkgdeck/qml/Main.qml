import QtQuick
import Qt.labs.platform as Platform
import io.github.astrovm.PkgDeck

Browser {
    id: browser
    backend: PackageController {}
    // The Mac app's native notifications and Dock badge (see native/macos.h);
    // null elsewhere, where the tray shows notifications.
    readonly property var mac: typeof macNative !== "undefined" ? macNative : null
    trayAvailable: tray.available
    notificationAvailable: mac ? mac.notificationsAllowed : tray.available && tray.supportsMessages
    function notify(title, body) {
        if (mac)
            mac.notify(title, body);
        else
            tray.showMessage(title, body, Platform.SystemTrayIcon.Information, 8000);
    }
    onTestNotificationRequested: notify("PkgDeck test", "Desktop notifications are working.")
    Platform.SystemTrayIcon {
        id: tray
        visible: browser.backgroundMode && available
        // The Mac menu bar tints a black template icon for light and dark bars.
        icon.source: browser.macOS ? "qrc:/pkgdeck/logo-template.svg" : browser.logoIconSource
        icon.mask: browser.macOS
        tooltip: "PkgDeck"
        onActivated: function(reason) {
            if (reason === Platform.SystemTrayIcon.Trigger)
                browser.toggleFromTray();
        }
        // Hidden until the tray opens it. Plasma's tray menu is a QMenu, and a
        // visible one pops up at startup; on Wayland that fails with no
        // parent window ("Failed to create grabbing popup").
        menu: Platform.Menu {
            visible: false
            Platform.MenuItem { text: "Open"; onTriggered: browser.showFromTray() }
            Platform.MenuItem { text: "Check now"; onTriggered: browser.checkUpdates(true) }
            Platform.MenuItem { text: "Quit"; onTriggered: { browser.forceQuit = true; Qt.quit(); } }
        }
        onMessageClicked: { browser.showFromTray(); browser.openView("Updates"); }
    }
    // The Mac app menu: About, Settings and Quit go where macOS puts them,
    // and Quit really quits instead of hiding to the menu bar.
    Instantiator {
        active: browser.macOS
        delegate: Platform.MenuBar {
            window: browser
            Platform.Menu {
                title: "Window"
                Platform.MenuItem { text: "About PkgDeck"; role: Platform.MenuItem.AboutRole; onTriggered: { browser.showFromTray(); browser.openView("Settings"); } }
                Platform.MenuItem { text: "Settings…"; role: Platform.MenuItem.PreferencesRole; onTriggered: { browser.showFromTray(); browser.openView("Settings"); } }
                Platform.MenuItem { text: "Quit PkgDeck"; role: Platform.MenuItem.QuitRole; onTriggered: browser.quitFromKeyboard() }
                Platform.MenuItem { text: "Minimize"; onTriggered: browser.showMinimized() }
                Platform.MenuItem { text: "Close Window"; onTriggered: browser.close() }
                Platform.MenuItem { text: "Show PkgDeck"; onTriggered: browser.showFromTray() }
            }
        }
    }
    // Clicking the Dock icon brings back a window hidden to the menu bar.
    Connections {
        target: Qt.application
        enabled: browser.macOS
        function onStateChanged() {
            if (Qt.application.state === Qt.ApplicationActive && !browser.visible)
                browser.showFromTray();
        }
    }
    Connections {
        target: browser
        function onBackgroundStateChanged() {
            const count = browser.backgroundState.available || 0;
            if (browser.mac)
                browser.mac.setBadge(count > 0 ? String(count) : "");
            if (browser.backgroundState.notify && tray.visible && browser.notificationAvailable) {
                browser.notify("PkgDeck updates", count + (count === 1 ? " update" : " updates") + " available");
                browser.backend.acknowledgeNotification();
            }
        }
    }
    Connections {
        target: browser.mac
        function onNotificationClicked() { browser.showFromTray(); browser.openView("Updates"); }
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
