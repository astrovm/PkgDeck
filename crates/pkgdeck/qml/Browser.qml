import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import QtCore
import org.kde.kirigami as Kirigami

Controls.ApplicationWindow {
    id: root
    required property var backend
    property string currentView: "Discover"
    property string resultView: "Discover"
    property var items: currentView === resultView ? JSON.parse(backend.rows || "[]") : []
    property var detail: JSON.parse(backend.details || "{}")
    property bool closePending: false
    property var selected: results.currentIndex >= 0 && results.currentIndex < items.length ? items[results.currentIndex] : null
    property string source: argument("--from", preferences.source)
    property bool useSudo: argument("--auth", preferences.authorization) === "sudo"
    readonly property bool compact: width < 760
    readonly property bool dark: preferences.appearance === 1 || (preferences.appearance === 0 && Qt.styleHints.colorScheme === Qt.Dark)
    readonly property color canvas: dark ? "#111820" : "#f3f5f8"
    readonly property color surface: dark ? "#1b2531" : "#ffffff"
    readonly property color ink: dark ? "#ecf1f8" : "#1c2b3e"
    readonly property color muted: dark ? "#a2b1c4" : "#57677e"
    readonly property color line: dark ? "#334153" : "#dce3ec"
    readonly property color accent: dark ? "#80b6ff" : "#245fc6"
    readonly property color selection: dark ? "#283e59" : "#e8f0ff"
    color: canvas
    font.family: "sans-serif"
    font.pixelSize: 14
    palette.window: canvas
    palette.base: surface
    palette.text: ink
    palette.windowText: ink
    palette.buttonText: ink
    palette.button: surface
    palette.highlight: accent
    palette.highlightedText: dark ? "#111820" : "#ffffff"

    component ActionButton: Controls.Button {
        id: control
        property bool primary: false
        property bool navigation: false
        property string symbol: "package"
        Accessible.name: text
        implicitHeight: 38
        horizontalPadding: 16
        opacity: enabled ? 1 : 0.45
        background: Rectangle {
            radius: 7
            color: control.primary && control.enabled ? root.accent : (control.hovered ? root.selection : root.surface)
            border.color: control.activeFocus ? root.accent : (control.navigation ? "transparent" : root.line)
            border.width: control.activeFocus ? 2 : 1
        }
        contentItem: RowLayout {
            spacing: 8
            DeckIcon {
                name: control.symbol
                ink: control.primary && control.enabled ? (root.dark ? "#111820" : "#ffffff") : root.ink
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
            }
            Text {
                text: control.text
                font: control.font
                color: control.primary && control.enabled ? (root.dark ? "#111820" : "#ffffff") : root.ink
                Layout.fillWidth: control.navigation
                verticalAlignment: Text.AlignVCenter
            }
        }
    }

    property url repositoryIconSource: root.dark ? "qrc:/pkgdeck/github-dark.svg" : "qrc:/pkgdeck/github.svg"
    readonly property url repositoryUrl: "https://github.com/astrovm/PkgDeck"
    width: 1100
    height: 760
    minimumWidth: 360
    minimumHeight: 400
    visible: true
    title: "PkgDeck — " + currentView + (backend.busy ? " — Working" : "")
    function argument(name, fallback) {
        const index = Qt.application.arguments.indexOf(name);
        return index >= 0 ? Qt.application.arguments[index + 1] : fallback;
    }
    function openView(view) {
        if (backend.busy)
            return;
        currentView = view;
        results.currentIndex = -1;
        if (view === "Search")
            search.forceActiveFocus();
        else if (["Discover", "Installed", "Updates", "Sources"].indexOf(view) >= 0)
            reload();
    }
    function reload() {
        resultView = currentView;
        results.currentIndex = -1;
        backend.load(currentView, search.text, source, useSudo);
    }
    function choose(index) {
        if (backend.busy || index < 0 || index >= items.length)
            return;
        results.currentIndex = index;
        backend.select(index);
    }
    function propose(action) {
        backend.propose(action, results.currentIndex);
    }
    Settings {
        id: preferences
        category: "Browser"
        property int appearance: 0
        property string source: ""
        property string authorization: "polkit"
    }
    onClosing: function (close) {
        if (backend.busy) {
            close.accepted = false;
            closePending = true;
            backend.cancel();
        }
    }
    Connections {
        target: backend
        function onBusyChanged() {
            if (!backend.busy && root.closePending)
                root.close();
            else if (!backend.busy && root.selected !== null)
                results.forceActiveFocus();
        }
        function onRowsChanged() {
            results.currentIndex = -1;
        }
        function onConfirmationChanged() {
            if (backend.confirmation.length)
                confirmation.open();
            else
                confirmation.close();
        }
    }
    Timer {
        interval: 40
        running: backend.busy
        repeat: true
        onTriggered: backend.poll()
    }
    Component.onCompleted: reload()

    RowLayout {
        anchors.fill: parent
        spacing: 0
        Rectangle {
            Layout.fillHeight: true
            Layout.preferredWidth: 196
            visible: !root.compact
            color: root.surface
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 16
                spacing: 8
                Controls.Label {
                    text: "PkgDeck"
                    font.pixelSize: 25
                    font.bold: true
                    color: root.ink
                    Layout.topMargin: 12
                }
                Item { Layout.preferredHeight: 24 }
                Repeater {
                    model: ["Discover", "Search", "Installed", "Updates", "Sources", "Settings", "Help / About"]
                    delegate: ActionButton {
                        required property string modelData
                        Layout.fillWidth: true
                        text: modelData
                        symbol: ({"Discover":"discover", "Search":"search", "Installed":"installed", "Updates":"updates", "Sources":"sources", "Settings":"settings", "Help / About":"help"})[modelData]
                        enabled: !backend.busy
                        navigation: true
                        primary: root.currentView === modelData
                        onClicked: root.openView(modelData)
                    }
                }
                Item { Layout.fillHeight: true }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 4
                    Controls.Label { text: "Made with"; color: root.muted; font.pixelSize: 11 }
                    DeckIcon { name: "heart"; ink: "#e34b5f"; Layout.preferredWidth: 14; Layout.preferredHeight: 14 }
                    Controls.Label { text: "by astro"; color: root.muted; font.pixelSize: 11 }
                    Controls.ToolButton {
                        objectName: "repositoryLink"
                        implicitWidth: 28
                        implicitHeight: 28
                        Accessible.name: "Open PkgDeck on GitHub"
                        Controls.ToolTip.visible: hovered
                        Controls.ToolTip.text: root.repositoryUrl
                        onClicked: Qt.openUrlExternally(root.repositoryUrl)
                        background: Rectangle {
                            radius: 5
                            color: parent.hovered ? root.selection : "transparent"
                            border.color: parent.activeFocus ? root.accent : "transparent"
                        }
                        contentItem: Image {
                            objectName: "repositoryIcon"
                            source: root.repositoryIconSource
                            sourceSize.width: 18
                            sourceSize.height: 18
                            fillMode: Image.PreserveAspectFit
                            Accessible.ignored: true
                        }
                    }
                }

            }
        }
        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: root.compact ? 12 : 28
            spacing: 14
            RowLayout {
                Layout.fillWidth: true
                Controls.ComboBox {
                    visible: root.compact
                    model: ["Discover", "Search", "Installed", "Updates", "Sources", "Settings", "Help / About"]
                    currentIndex: model.indexOf(root.currentView)
                    enabled: !backend.busy
                    onActivated: root.openView(currentText)
                    Accessible.name: "Navigation"
                    Layout.fillWidth: true
                }
                Kirigami.Heading {
                    visible: !root.compact
                    text: root.currentView
                    color: root.ink
                    level: 1
                    font.pointSize: 21
                    font.bold: true
                    Layout.fillWidth: true
                }
                Controls.Label {
                    visible: !root.compact
                    text: root.source ? root.source.toUpperCase() : "ALL SOURCES"
                    color: root.muted
                    font.pixelSize: 11
                    font.letterSpacing: 1
                }
            }
            RowLayout {
                Layout.fillWidth: true
                visible: ["Search", "Discover"].indexOf(root.currentView) >= 0
                Controls.TextField {
                    id: search
                    objectName: "searchField"
                    Layout.fillWidth: true
                    placeholderText: "Search package names and summaries"
                    Accessible.name: "Search packages"
                    enabled: !backend.busy
                    selectByMouse: true
                    implicitHeight: 44
                    color: root.ink
                    placeholderTextColor: root.muted
                    leftPadding: 14
                    background: Rectangle {
                        color: root.surface
                        radius: 8
                        border.color: search.activeFocus ? root.accent : root.line
                        border.width: search.activeFocus ? 2 : 1
                    }
                    onAccepted: {
                        root.currentView = "Search";
                        root.reload();
                    }
                }
                ActionButton {
                    text: "Search"
                    symbol: "search"
                    primary: true
                    enabled: !backend.busy && search.text.trim().length > 0
                    onClicked: { root.currentView = "Search"; root.reload(); }
                }
            }
            Controls.Label {
                visible: root.currentView === "Discover"
                text: "Search packages or select a source to inspect its availability."
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            ColumnLayout {
                visible: root.currentView === "Settings"
                Layout.fillWidth: true
                Controls.Label { text: "Appearance" }
                Controls.ComboBox {
                    objectName: "appearanceSetting"
                    model: ["System", "Dark", "Light"]
                    currentIndex: preferences.appearance
                    onActivated: preferences.appearance = currentIndex
                    Accessible.name: "Appearance"
                }
                Controls.Label {
                    text: "Package source"
                }
                Controls.ComboBox {
                    objectName: "sourceSetting"
                    model: ["All available sources", "APT", "Homebrew", "AppImage", "Flatpak"]
                    currentIndex: ["", "apt", "homebrew", "appimage", "flatpak"].indexOf(root.source)
                    onActivated: {
                        root.source = ["", "apt", "homebrew", "appimage", "flatpak"][currentIndex];
                        preferences.source = root.source;
                    }
                    Accessible.name: "Package source"
                }
                Controls.Label {
                    text: "APT authorization"
                }
                Controls.ComboBox {
                    objectName: "authorizationSetting"
                    model: ["Host polkit agent", "Existing sudo credentials"]
                    currentIndex: root.useSudo ? 1 : 0
                    onActivated: {
                        root.useSudo = currentIndex === 1;
                        preferences.authorization = root.useSudo ? "sudo" : "polkit";
                    }
                    Accessible.name: "APT authorization"
                }
                Controls.Label {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    text: "Polkit uses your host's authentication agent. Sudo requires an existing grant; passwords are never collected here. Homebrew always runs unprivileged. Choose the appearance below, or follow your system theme."
                }
            }
            Controls.Label {
                visible: root.currentView === "Help / About"
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                text: "PkgDeck 0.1.0\nA unified package interface for Linux.\n\nCtrl+F: search • Ctrl+1: Discover • Ctrl+3: Installed • Ctrl+4: Updates • Ctrl+5: Sources\nCtrl+L: focus results • Up/Down: select • Ctrl+I: install • Ctrl+D: remove • Ctrl+U: upgrade • Ctrl+M: refresh source • Ctrl+R: reload\nEscape: cancel current work\n\nRefresh updates source metadata; Upgrade changes an installed package. Writes require confirmation and may change native dependencies. Cancellation waits for a native write already running.\n\nAPT and Homebrew are supported. Flatpak and Snap host operations remain disabled."
                textFormat: Text.PlainText
            }
            Rectangle {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.minimumHeight: 130
                visible: root.currentView !== "Settings" && root.currentView !== "Help / About"
                color: root.surface
                radius: 10
                border.color: results.activeFocus ? root.accent : root.line
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 1
                    spacing: 0
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.margins: 14
                        Controls.Label {
                            text: root.items.length + (root.currentView === "Sources" || root.currentView === "Discover" ? (root.items.length === 1 ? " source" : " sources") : (root.items.length === 1 ? " package" : " packages"))
                            color: root.muted
                            font.pixelSize: 12
                            Layout.fillWidth: true
                        }
                        Controls.Label { text: "↑ ↓ Select"; color: root.muted; font.pixelSize: 12 }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: root.line }
                    RowLayout {
                        visible: !root.compact
                        spacing: 14
                        Layout.fillWidth: true
                        Layout.leftMargin: 16
                        Layout.rightMargin: 16
                        Layout.topMargin: 10
                        Layout.bottomMargin: 10
                        Controls.Label { text: "NAME / SOURCE"; color: root.muted; font.pixelSize: 11; Layout.preferredWidth: 202 }
                        Controls.Label { text: "VERSION"; color: root.muted; font.pixelSize: 11; Layout.preferredWidth: 150 }
                        Controls.Label { text: "SUMMARY"; color: root.muted; font.pixelSize: 11; Layout.fillWidth: true }
                    }
                    ListView {
                        id: results
                        objectName: "packageResults"
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        model: root.items
                        clip: true
                        reuseItems: true
                        currentIndex: -1
                        onCountChanged: currentIndex = -1
                        keyNavigationEnabled: false
                        activeFocusOnTab: true
                        Controls.ScrollBar.vertical: Controls.ScrollBar {}
                        Keys.onDownPressed: root.choose(Math.min(count - 1, currentIndex + 1))
                        Keys.onUpPressed: root.choose(Math.max(0, currentIndex - 1))
                        delegate: Controls.ItemDelegate {
                            id: packageRow
                            required property var modelData
                            required property int index
                            width: ListView.view.width
                            height: root.compact ? 78 : 56
                            leftPadding: 16
                            rightPadding: 16
                            highlighted: results.currentIndex === index
                            enabled: !backend.busy
                            Accessible.name: modelData.name + ", " + modelData.source + ", " + (modelData.summary || "")
                            onClicked: { results.forceActiveFocus(); root.choose(index); }
                            background: Rectangle {
                                color: packageRow.highlighted ? root.selection : (packageRow.hovered ? root.canvas : "transparent")
                                Rectangle { width: 3; height: parent.height; visible: packageRow.highlighted; color: root.accent }
                                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: root.line; opacity: 0.5 }
                            }
                            contentItem: RowLayout {
                                spacing: 14
                                ColumnLayout {
                                    spacing: 4
                                    Layout.preferredWidth: root.compact ? -1 : 202
                                    Layout.fillWidth: root.compact
                                    Controls.Label {
                                        text: modelData.name
                                        color: root.ink
                                        font.bold: true
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: 6
                                        DeckIcon {
                                            name: modelData.source
                                            ink: root.muted
                                            Layout.preferredWidth: 14
                                            Layout.preferredHeight: 14
                                        }
                                        Controls.Label {
                                        text: modelData.source.toUpperCase() + (modelData.architecture ? " · " + modelData.architecture : "")
                                        color: root.muted
                                        font.pixelSize: 11
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                        }
                                    }
                                    Controls.Label {
                                        visible: root.compact
                                        text: modelData.summary || ""
                                        color: root.muted
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                }
                                Controls.Label {
                                    Layout.preferredWidth: root.compact ? 100 : 150
                                    text: modelData.kind === "source" ? (modelData.available ? "Available" : "Unavailable") : (modelData.installed ? modelData.installed + (modelData.update === "available" ? " → " + modelData.candidate : " · installed") : modelData.candidate || "Unknown")
                                    color: modelData.update === "available" ? root.accent : root.muted
                                    textFormat: Text.PlainText
                                    elide: Text.ElideRight
                                    font.pixelSize: 12
                                }
                                Controls.Label {
                                    visible: !root.compact
                                    Layout.fillWidth: true
                                    text: modelData.summary || ""
                                    color: root.muted
                                    textFormat: Text.PlainText
                                    elide: Text.ElideRight
                                }
                            }
                        }
                        Controls.Label {
                            anchors.centerIn: parent
                            width: parent.width - 32
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.WordWrap
                            color: root.muted
                            visible: results.count === 0
                            text: backend.busy ? "Loading packages…" : root.currentView === "Search" ? "Search by name or description.\nIf nothing matches, try a shorter search or another source." : "No results to show.\nCheck source availability or reload to try again."
                        }
                    }
                }
            }
            Rectangle {
                visible: root.selected !== null && ["Search", "Installed", "Updates", "Sources", "Discover"].indexOf(root.currentView) >= 0
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(root.height * 0.27, 180)
                color: root.surface
                radius: 10
                border.color: root.line
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 6
                    RowLayout {
                        Layout.fillWidth: true
                        DeckIcon {
                            name: root.selected && root.selected.kind === "source" ? root.selected.source : "package"
                            ink: root.accent
                            Layout.preferredWidth: 24
                            Layout.preferredHeight: 24
                        }
                        Controls.Label {
                        text: root.selected ? root.selected.name : ""
                        textFormat: Text.PlainText
                        color: root.ink
                        font.pixelSize: root.compact ? 18 : 22
                        font.bold: true
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                        }
                    }
                    Controls.ScrollView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        Controls.TextArea {
                            objectName: "packageDetails"
                            readOnly: true
                            selectByMouse: true
                            color: root.muted
                            padding: 0
                            background: null
                            wrapMode: TextEdit.Wrap
                            textFormat: TextEdit.PlainText
                            text: root.detail.package ? (root.detail.description || "") + "\n\nScope: " + (root.detail.package.scope_label || "Unknown") + "   ·   Homepage: " + (root.detail.homepage || "Unavailable") + "\nDependencies: " + ((root.detail.dependencies || []).join(", ") || "None listed") : (root.detail.availability || "") + "\n\nCapabilities: " + (root.detail.capabilities || []).join(", ")
                            Accessible.name: "Selected package or source details"
                        }
                    }
                }
            }
            Flow {
                Layout.fillWidth: true
                spacing: 8
                visible: ["Search", "Installed", "Updates", "Sources", "Discover"].indexOf(root.currentView) >= 0
                ActionButton {
                    objectName: "upgradeAllButton"
                    visible: root.currentView === "Updates"
                    text: "Upgrade all"
                    symbol: "updates"
                    primary: true
                    enabled: !backend.busy && backend.upgradable
                    onClicked: root.propose("upgrade-all")
                }
                ActionButton {
                    objectName: "installButton"
                    visible: root.selected !== null && root.selected.kind === "package" && !root.selected.installed
                    text: "Install"
                    symbol: "install"
                    primary: true
                    enabled: !backend.busy && root.selected !== null && root.selected.kind === "package" && !root.selected.installed
                    onClicked: root.propose("install")
                }
                ActionButton {
                    objectName: "removeButton"
                    visible: root.selected !== null && !!root.selected.installed
                    text: "Remove"
                    symbol: "remove"
                    enabled: !backend.busy && root.selected !== null && !!root.selected.installed
                    onClicked: root.propose("remove")
                }
                ActionButton {
                    objectName: "upgradeButton"
                    visible: root.selected !== null && root.selected.update === "available"
                    text: "Upgrade"
                    symbol: "updates"
                    primary: true
                    enabled: !backend.busy && root.selected !== null && root.selected.update === "available"
                    onClicked: root.propose("upgrade")
                }
                ActionButton {
                    objectName: "refreshButton"
                    visible: root.selected !== null && root.selected.kind === "source"
                    text: "Refresh source"
                    symbol: "refresh"
                    primary: true
                    enabled: !backend.busy && root.selected !== null && root.selected.kind === "source" && root.selected.available
                    onClicked: root.propose("refresh")
                }
                ActionButton {
                    text: "Reload"
                    symbol: "refresh"
                    enabled: !backend.busy
                    onClicked: root.reload()
                }
            }
            RowLayout {
                Layout.fillWidth: true
                DeckIcon {
                    name: "help"
                    ink: root.muted
                    visible: !backend.busy
                    Layout.alignment: Qt.AlignTop
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                }
                Controls.BusyIndicator {
                    running: backend.busy
                    visible: running
                    Layout.preferredWidth: 32
                    Layout.preferredHeight: 32
                }
                Controls.ScrollView {
                    Layout.fillWidth: true
                    Layout.preferredHeight: root.compact ? 42 : 44
                    Controls.TextArea {
                        objectName: "operationStatus"
                        color: root.muted
                        background: null
                        font.pixelSize: 12
                        padding: 0
                        text: backend.status
                        readOnly: true
                        selectByMouse: true
                        textFormat: TextEdit.PlainText
                        wrapMode: TextEdit.Wrap
                        Accessible.name: "Operation status"
                    }
                }
                ActionButton {
                    text: "Cancel"
                    symbol: "cancel"
                    visible: backend.busy
                    onClicked: backend.cancel()
                }
            }
        }
    }
    Controls.Dialog {
        id: confirmation
        objectName: "confirmationDialog"
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 600)
        height: Math.min(root.height - 32, 340)
        background: Rectangle { color: root.surface; radius: 12; border.color: root.line }
        title: "Confirm package operation"
        modal: true
        standardButtons: Controls.Dialog.Yes | Controls.Dialog.No
        onOpened: standardButton(Controls.Dialog.No).forceActiveFocus()
        Shortcut {
            sequence: "Alt+Y"
            enabled: confirmation.opened
            onActivated: confirmation.accept()
        }
        Shortcut {
            sequence: "Alt+N"
            enabled: confirmation.opened
            onActivated: confirmation.reject()
        }
        onAccepted: backend.confirm(true)
        onRejected: backend.confirm(false)
        contentItem: Controls.ScrollView {
            Controls.TextArea {
                text: backend.confirmation
                readOnly: true
                selectByMouse: true
                wrapMode: TextEdit.Wrap
                textFormat: TextEdit.PlainText
            }
        }
    }
    Shortcut {
        sequence: "Ctrl+Q"
        onActivated: root.close()
    }
    Shortcut {
        sequence: "Ctrl+L"
        enabled: !backend.busy
        onActivated: results.forceActiveFocus()
    }
    Shortcut {
        sequence: "Ctrl+F"
        enabled: !backend.busy
        onActivated: {
            root.openView("Search");
            search.selectAll();
        }
    }
    Shortcut {
        sequence: "Ctrl+1"
        onActivated: root.openView("Discover")
    }
    Shortcut {
        sequence: "Ctrl+3"
        onActivated: root.openView("Installed")
    }
    Shortcut {
        sequence: "Ctrl+4"
        onActivated: root.openView("Updates")
    }
    Shortcut {
        sequence: "Ctrl+5"
        onActivated: root.openView("Sources")
    }
    Shortcut {
        sequence: "Ctrl+R"
        enabled: !backend.busy
        onActivated: root.reload()
    }
    Shortcut {
        sequence: "Ctrl+I"
        enabled: !backend.busy
        onActivated: root.propose("install")
    }
    Shortcut {
        sequence: "Ctrl+D"
        enabled: !backend.busy
        onActivated: root.propose("remove")
    }
    Shortcut {
        sequence: "Ctrl+U"
        enabled: !backend.busy
        onActivated: root.propose("upgrade")
    }
    Shortcut {
        sequence: "Ctrl+Shift+U"
        enabled: root.currentView === "Updates" && !backend.busy && backend.upgradable
        onActivated: root.propose("upgrade-all")
    }
    Shortcut {
        sequence: "Ctrl+M"
        enabled: !backend.busy
        onActivated: root.propose("refresh")
    }
    Shortcut {
        sequence: "Escape"
        enabled: backend.busy
        onActivated: backend.cancel()
    }
}
