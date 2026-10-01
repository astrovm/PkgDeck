import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

// The Settings page. `app` is the main window, which owns the stored
// preferences (`app.store`) and the backend.
DeckScrollView {
    id: page
    required property var app
    readonly property var store: app.store
    // Narrow pages put each label above its control.
    readonly property bool stacked: availableWidth < 520
    readonly property int cardWidth: 760
    ink: Theme.muted
    objectName: "settingsScroll"
    contentWidth: availableWidth
    clip: true

    // One setting: label (with an optional hint) and its control, side by
    // side on wide pages and stacked on narrow ones. The control is the
    // row's second child and sizes itself with controlWidth().
    component SettingRow: GridLayout {
        id: row
        property string label: ""
        property string hint: ""
        Layout.fillWidth: true
        columns: page.stacked ? 1 : 2
        columnSpacing: 12
        rowSpacing: 6
        ColumnLayout {
            spacing: 2
            Layout.fillWidth: true
            Layout.minimumWidth: page.stacked ? 0 : 120
            Controls.Label {
                text: row.label
                color: Theme.ink
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            Controls.Label {
                text: row.hint
                visible: text.length > 0
                color: Theme.muted
                font.pointSize: Theme.pointSize(Theme.smallScale)
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
        }
    }

    function controlWidth() { return stacked ? -1 : 240; }

    ColumnLayout {
        width: Math.min(page.availableWidth, page.cardWidth)
        spacing: 14
        SettingsCard {
            title: "Appearance"
            SettingRow {
                label: "Theme"
                ThemedComboBox {
                    objectName: "appearanceSetting"
                    Layout.fillWidth: page.stacked
                    Layout.preferredWidth: page.controlWidth()
                    model: ["System", "Dark", "Light"]
                    currentIndex: page.store.appearance
                    onActivated: page.store.appearance = currentIndex
                    Accessible.name: "Appearance"
                }
            }
            SettingCheckBox {
                objectName: "animationsSetting"
                text: "Animations"
                checked: !page.app.reduceMotion
                onToggled: page.app.reduceMotion = !checked
                Accessible.name: "Enable interface animations"
            }
        }
        SettingsCard {
            title: "Update checks"
            SettingCheckBox {
                objectName: "backgroundModeSetting"
                text: "Background checks"
                checked: page.store.backgroundMode
                onClicked: {
                    if (!checked && page.store.autostart && !page.app.backend.setAutostart(false)) {
                        checked = true;
                        return;
                    }
                    page.store.backgroundMode = checked;
                    if (!checked)
                        page.store.autostart = false;
                }
                Accessible.name: text
            }
            SettingRow {
                label: "Check every"
                ThemedComboBox {
                    objectName: "checkIntervalSetting"
                    readonly property var minutes: [15, 30, 60, 180, 360, 720, 1440]
                    Layout.fillWidth: page.stacked
                    Layout.preferredWidth: page.controlWidth()
                    enabled: page.store.backgroundMode
                    model: ["15 minutes", "30 minutes", "Hour", "3 hours", "6 hours", "12 hours", "Day"]
                    currentIndex: Math.max(0, minutes.indexOf(page.store.checkInterval))
                    onActivated: page.store.checkInterval = minutes[currentIndex]
                    Accessible.name: "Check for updates every"
                }
            }
            SettingCheckBox {
                objectName: "autoUpdateSetting"
                text: "Install updates automatically"
                enabled: page.store.backgroundMode
                checked: page.store.autoUpdate
                onClicked: page.store.autoUpdate = checked
                Accessible.name: text
            }
            SettingCheckBox {
                objectName: "autoUpdateRemovalsSetting"
                text: "Allow updates that remove packages"
                enabled: page.store.backgroundMode && page.store.autoUpdate
                checked: page.store.autoUpdateRemovals
                onClicked: page.store.autoUpdateRemovals = checked
                Accessible.name: text
            }
            Controls.Label {
                objectName: "autoUpdateRemovalsHelp"
                Layout.fillWidth: true
                Layout.leftMargin: 28
                wrapMode: Text.WordWrap
                color: Theme.muted
                text: page.store.autoUpdateRemovals
                    ? "An update can remove packages when the package manager decides to, such as an old kernel replaced by a new one."
                    : "An update that would remove packages waits for you to run Update all."
            }
            SettingCheckBox {
                objectName: "systemApprovalSetting"
                text: "Allow system updates without a password"
                enabled: page.store.backgroundMode && page.store.autoUpdate
                checked: page.store.systemApproval !== ""
                onClicked: {
                    page.app.backend.allowSystemUpdates(checked);
                    // The saved setting decides once the password prompt is answered.
                    checked = Qt.binding(() => page.store.systemApproval !== "");
                }
                Accessible.name: text
            }
            Controls.Label {
                objectName: "systemApprovalHelp"
                Layout.fillWidth: true
                Layout.leftMargin: 28
                wrapMode: Text.WordWrap
                color: page.app.backend.approval_error ? Theme.danger : Theme.muted
                text: page.app.backend.approval_error
                    || (page.app.macOS
                        ? "MacPorts updates run without asking for your password. Asks for it once to allow this."
                        : "System packages (APT, DNF, Pacman, Zypper, system Flatpaks and Snaps) update without asking for your password, and only updates: nothing is installed or removed. Asks for it once to allow this.")
            }
            SettingCheckBox {
                objectName: "autostartSetting"
                text: "Start in background at login"
                visible: page.app.desktopAutostartSupported
                checked: page.store.autostart
                enabled: page.store.backgroundMode && page.app.trayAvailable
                onClicked: {
                    if (page.app.backend.setAutostart(checked))
                        page.store.autostart = checked;
                    else
                        checked = page.store.autostart;
                }
                Accessible.name: text
            }
            Rectangle { Layout.fillWidth: true; Layout.preferredHeight: 1; color: Theme.line }
            GridLayout {
                Layout.fillWidth: true
                columns: page.stacked ? 1 : 2
                columnSpacing: 12
                rowSpacing: 10
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.minimumWidth: page.stacked ? 0 : 160
                    spacing: 3
                    Controls.Label {
                        objectName: "backgroundCheckStatus"
                        readonly property int found: page.app.backgroundState.available || 0
                        text: page.app.backgroundState.last_check
                            ? "Last check: " + new Date(page.app.backgroundState.last_check * 1000).toLocaleString(Qt.locale(), Locale.ShortFormat)
                                + ", " + found + (found === 1 ? " update found" : " updates found")
                            : "Last check: never"
                        color: Theme.muted
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                    Controls.Label {
                        objectName: "notificationAvailability"
                        text: !page.app.trayAvailable ? "Notifications unavailable: no " + (page.app.macOS ? "menu bar icon" : "system tray") + " found"
                            : !page.app.notificationAvailable ? (page.app.macOS ? "Notifications are off: allow PkgDeck in System Settings > Notifications" : "This system tray does not support notifications")
                            : page.app.notificationPermissionNeeded ? "macOS hasn't allowed PkgDeck's notifications yet, so they show Script Editor's icon. Allow PkgDeck in Notification settings."
                            : "Notifications available"
                        color: Theme.muted
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                    ActionButton {
                        objectName: "notificationSettingsButton"
                        text: "Notification settings"
                        symbol: "settings"
                        visible: page.app.notificationPermissionNeeded
                        onClicked: page.app.notificationSettingsRequested()
                    }
                }
                ActionButton {
                    objectName: "testNotificationButton"
                    text: "Test notification"
                    symbol: "bell"
                    enabled: page.store.backgroundMode && page.app.notificationAvailable
                    onClicked: page.app.testNotificationRequested()
                    Layout.alignment: page.stacked ? Qt.AlignLeft : (Qt.AlignRight | Qt.AlignVCenter)
                }
            }
        }
        SettingsCard {
            title: "About"
            GridLayout {
                Layout.fillWidth: true
                columns: page.stacked ? 2 : 3
                columnSpacing: 12
                rowSpacing: 10
                Image {
                    source: page.app.logoIconSource
                    sourceSize.width: 36
                    sourceSize.height: 36
                    Accessible.ignored: true
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 2
                    Controls.Label {
                        objectName: "aboutText"
                        text: "PkgDeck " + page.app.backend.version
                        color: Theme.ink
                        font.weight: Font.DemiBold
                        font.pointSize: Theme.pointSize(1.15)
                        Layout.fillWidth: true
                    }
                    RowLayout {
                        objectName: "compactSignature"
                        visible: page.app.sidebarRail
                        spacing: 4
                        Controls.Label { text: "Made with"; color: Theme.muted }
                        DeckIcon { name: "heart"; ink: Theme.heart; Layout.preferredWidth: 13; Layout.preferredHeight: 13 }
                        Controls.Label { text: "by astro"; color: Theme.muted }
                    }
                }
                ActionButton {
                    objectName: "repositoryLink"
                    text: "GitHub"
                    symbol: ""
                    iconSource: page.app.repositoryIconSource
                    Accessible.name: "Open PkgDeck on GitHub"
                    onClicked: Qt.openUrlExternally(page.app.repositoryUrl)
                    Layout.columnSpan: page.stacked ? 2 : 1
                }
            }
        }
        SettingsCard {
            title: "Keyboard shortcuts"
            GridLayout {
                columns: page.availableWidth < 620 ? 1 : 2
                columnSpacing: 28
                rowSpacing: 6
                Layout.fillWidth: true
                Repeater {
                    objectName: "aboutShortcuts"
                    model: [
                        {action: "Search", keys: "Ctrl+1"},
                        {action: "Installed", keys: "Ctrl+2"},
                        {action: "Updates", keys: "Ctrl+3"},
                        {action: "Clean", keys: "Ctrl+4"},
                        {action: "Sources", keys: "Ctrl+5"},
                        {action: "Search or filter", keys: "Ctrl+F"},
                        {action: "Settings", keys: "Ctrl+,"},
                        {action: "Activity", keys: "Ctrl+J"},
                        {action: "Focus results", keys: "Ctrl+L"},
                        {action: "Select result", keys: "↑ / ↓"},
                        {action: "Install, remove or update the selected row", keys: "Enter"},
                        {action: "Update selected", keys: "Ctrl+Shift+U"},
                        {action: "Install", keys: "Ctrl+I"},
                        {action: "Remove", keys: "Ctrl+D"},
                        {action: "Update", keys: "Ctrl+U"},
                        {action: "Refresh the source's package lists", keys: page.app.refreshListsKeys},
                        {action: "Reload the page", keys: "Ctrl+R"},
                        {action: "Apply a confirmation", keys: "Ctrl+Enter"},
                        {action: "Clear search or close details", keys: "Esc"},
                        {action: "Quit", keys: "Ctrl+Q"}
                    ]
                    delegate: RowLayout {
                        required property var modelData
                        Layout.fillWidth: true
                        Layout.minimumWidth: page.availableWidth < 620 ? 0 : 240
                        spacing: 12
                        Controls.Label {
                            text: modelData.action
                            color: Theme.muted
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                        Controls.Label {
                            text: page.app.keys(modelData.keys)
                            color: Theme.ink
                            font.family: "monospace"
                            font.pointSize: Theme.pointSize(0.88)
                            leftPadding: 7
                            rightPadding: 7
                            topPadding: 2
                            bottomPadding: 2
                            background: Rectangle {
                                radius: Theme.smallRadius
                                color: Theme.canvas
                                border.color: Theme.line
                            }
                        }
                    }
                }
            }
        }
        Item { Layout.preferredHeight: 4 }
    }
}
