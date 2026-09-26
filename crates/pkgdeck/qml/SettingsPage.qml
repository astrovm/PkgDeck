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
                        text: !page.app.trayAvailable ? "Notifications unavailable: no system tray found"
                            : !page.app.notificationAvailable ? "This system tray does not support notifications"
                            : "Notifications available"
                        color: Theme.muted
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
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
            title: "Authentication"
            visible: page.app.systemAuthorizationSupported
            SettingRow {
                label: "Ask for permission with"
                hint: page.app.useSudo
                    ? "Uses a sudo login you already started in a terminal (for example with sudo -v)."
                    : "Your desktop asks for your password when a change needs it."
                ThemedComboBox {
                    objectName: "authorizationSetting"
                    Layout.fillWidth: page.stacked
                    Layout.preferredWidth: page.controlWidth()
                    model: ["System prompt", "Existing sudo session"]
                    currentIndex: page.app.useSudo ? 1 : 0
                    onActivated: {
                        page.app.useSudo = currentIndex === 1;
                        page.store.authorization = page.app.useSudo ? "sudo" : "polkit";
                    }
                    Accessible.name: "Authentication"
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
                        {action: "Refresh the source's package lists", keys: "Ctrl+M"},
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
                            text: modelData.keys
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
