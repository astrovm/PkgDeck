import QtQuick
import Qt.labs.platform as Platform
import io.github.astrovm.PkgDeck

Browser {
    id: browser
    backend: PackageController {}
    // The Mac app's native menu bar, notifications and Dock badge (see native/macos.h);
    // null elsewhere, where the tray shows notifications.
    readonly property var mac: typeof macNative !== "undefined" ? macNative : null
    readonly property var tray: trayLoader.object
    trayAvailable: mac ? mac.trayAvailable : !!tray && tray.available
    notificationAvailable: mac ? mac.notificationsAllowed : !!tray && tray.available && tray.supportsMessages
    notificationPermissionNeeded: mac ? mac.notificationsAllowed && !mac.authorized : false
    function notify(title, body) {
        if (mac)
            mac.notify(title, body);
        else if (tray)
            tray.showMessage(title, body, Platform.SystemTrayIcon.Information, 8000);
    }
    onTestNotificationRequested: {
        // Shows the system prompt again while the person hasn't decided.
        if (mac && !mac.authorized)
            mac.requestPermission();
        notify("PkgDeck test", "Desktop notifications are working.");
    }
    onNotificationSettingsRequested: if (mac) mac.openNotificationSettings()
    // Notification permission can change in System Settings while PkgDeck runs.
    Connections {
        target: Qt.application
        function onStateChanged() {
            if (browser.mac && Qt.application.state === Qt.ApplicationActive)
                browser.mac.refreshPermission();
        }
    }
    Binding {
        target: browser.mac
        property: "trayVisible"
        value: browser.backgroundMode
        when: browser.mac !== null
    }
    // Qt 6.11.2's Cocoa tray reads NSEvent.clickCount as the menu opens.
    // On macOS 27 that event is not always a mouse event, and the read
    // aborts the process. MacNative owns the status item instead.
    Instantiator {
        id: trayLoader
        active: !browser.macOS
        delegate: Platform.SystemTrayIcon {
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
            if (browser.backgroundState.notify && browser.backgroundMode && browser.trayAvailable && browser.notificationAvailable) {
                browser.notify("PkgDeck updates", count + (count === 1 ? " update" : " updates") + " available");
                browser.backend.acknowledgeNotification();
            }
        }
    }
    Connections {
        target: browser.mac
        function onTrayOpenRequested() { browser.showFromTray(); }
        function onTrayCheckRequested() { browser.checkUpdates(true); }
        function onTrayQuitRequested() { browser.quitFromKeyboard(); }
        function onNotificationClicked() { browser.showFromTray(); browser.openView("Updates"); }
    }
    Timer {
        interval: 150
        running: Qt.application.arguments.indexOf("--smoke-test") !== -1
        onTriggered: {
            // Opening Qt's Cocoa tray menu reads NSEvent.clickCount and aborts
            // on macOS 27. This process must not construct that tray, and the
            // native menu must survive being opened.
            if (browser.macOS && (trayLoader.active || trayLoader.object !== null)) {
                console.error("PKGDECK_QT_TRAY");
                Qt.exit(1);
                return;
            }
            if (browser.mac && browser.mac.trayAvailable) {
                const menu = browser.mac.exerciseTrayMenu();
                if (menu !== "Open\nCheck now\nQuit") {
                    console.error("PKGDECK_TRAY_MENU " + menu);
                    Qt.exit(1);
                    return;
                }
                console.info("PKGDECK_TRAY_MENU Open|Check now|Quit");
            }
            console.info("PKGDECK_GUI_READY");
            Qt.quit();
        }
    }
}
