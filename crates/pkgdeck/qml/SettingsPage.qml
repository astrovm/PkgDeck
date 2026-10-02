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
                Controls.Slider {
                    id: interval
                    objectName: "checkIntervalSetting"
                    readonly property var minutes: [15, 30, 60, 180, 360, 720, 1440, 2880, 4320, 10080]
                    readonly property var labels: ["15 minutes", "30 minutes", "1 hour", "3 hours", "6 hours", "12 hours", "1 day", "2 days", "3 days", "1 week"]
                    readonly property string currentText: labels[Math.round(value)]
                    // The step closest to the saved interval.
                    function nearest(saved) {
                        let best = 0;
                        for (let i = 1; i < minutes.length; i++)
                            if (Math.abs(minutes[i] - saved) < Math.abs(minutes[best] - saved))
                                best = i;
                        return best;
                    }
                    Layout.fillWidth: page.stacked
                    Layout.preferredWidth: page.controlWidth()
                    enabled: page.store.backgroundMode
                    from: 0
                    to: minutes.length - 1
                    stepSize: 1
                    snapMode: Controls.Slider.SnapAlways
                    value: nearest(page.store.checkInterval)
                    onMoved: {
                        page.store.checkInterval = minutes[Math.round(value)];
                        // Dragging replaced the binding; follow the saved setting again.
                        value = Qt.binding(() => nearest(page.store.checkInterval));
                    }
                    Accessible.name: "Check for updates every"
                    Accessible.description: currentText
                    leftPadding: 0
                    rightPadding: 0
                    topPadding: 22
                    background: Item {
                        x: interval.leftPadding
                        width: interval.availableWidth
                        height: interval.height
                        opacity: interval.enabled ? 1 : 0.45
                        Text {
                            objectName: "checkIntervalValue"
                            text: interval.currentText
                            color: Theme.ink
                            font: interval.font
                            anchors.right: parent.right
                            anchors.top: parent.top
                        }
                        Rectangle {
                            y: interval.topPadding + interval.availableHeight / 2 - height / 2
                            width: parent.width
                            height: 4
                            radius: 2
                            color: Theme.tint(Theme.ink, Theme.dark ? 0.18 : 0.16)
                            Rectangle {
                                width: interval.visualPosition * parent.width
                                height: parent.height
                                radius: 2
                                color: Theme.accent
                            }
                        }
                    }
                    handle: Rectangle {
                        x: interval.leftPadding + interval.visualPosition * (interval.availableWidth - width)
                        y: interval.topPadding + interval.availableHeight / 2 - height / 2
                        implicitWidth: 18
                        implicitHeight: 18
                        radius: 9
                        color: Theme.dark ? "#e9edf3" : "#ffffff"
                        border.color: interval.visualFocus ? Theme.accent : Theme.tint("#000000", 0.18)
                        border.width: interval.visualFocus ? 2 : 1
                        opacity: interval.enabled ? 1 : 0.45
                    }
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
                objectName: "allowRemovalsSetting"
                text: "Allow updates that remove packages"
                tooltipText: "Like an old kernel replaced by a new one"
                checked: page.store.allowRemovals
                onClicked: page.store.allowRemovals = checked
                Accessible.name: text
            }
            SettingCheckBox {
                objectName: "systemApprovalSetting"
                text: "Allow automatic updates without a password"
                tooltipText: "Changes you start still ask"
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
                visible: text.length > 0
                color: Theme.danger
                text: page.app.backend.approval_error
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
                    ActionButton {
                        objectName: "notificationSettingsButton"
                        text: "Notification settings"
                        symbol: "settings"
                        visible: page.app.notificationPermissionNeeded
                        onClicked: page.app.notificationSettingsRequested()
                    }
                }
                RowLayout {
                    spacing: 8
                    Layout.alignment: page.stacked ? Qt.AlignLeft : (Qt.AlignRight | Qt.AlignVCenter)
                    // A warning icon, only when notifications won't work as
                    // expected; hover it for why.
                    DeckIcon {
                        id: notificationWarning
                        objectName: "notificationAvailability"
                        readonly property string text: !page.app.trayAvailable ? "No " + (page.app.macOS ? "menu bar icon" : "system tray") + ", so no notifications"
                            : !page.app.notificationAvailable ? (page.app.macOS ? "Notifications are off in System Settings" : "This system tray can't show notifications")
                            : page.app.notificationPermissionNeeded ? "Notifications show Script Editor's icon until you allow PkgDeck"
                            : ""
                        name: "warning"
                        ink: Theme.warning
                        visible: text.length > 0
                        Layout.preferredWidth: 18
                        Layout.preferredHeight: 18
                        Accessible.ignored: false
                        Accessible.role: Accessible.StaticText
                        Accessible.name: text
                        HoverHandler { id: notificationWarningHover }
                        Controls.ToolTip.visible: notificationWarningHover.hovered
                        Controls.ToolTip.delay: 300
                        Controls.ToolTip.text: text
                    }
                    ActionButton {
                        objectName: "testNotificationButton"
                        text: "Test notification"
                        symbol: "bell"
                        enabled: page.store.backgroundMode && page.app.notificationAvailable
                        onClicked: page.app.testNotificationRequested()
                    }
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
