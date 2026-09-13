import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import QtCore
import org.kde.kirigami as Kirigami

Kirigami.ApplicationWindow {
    id: root
    required property var backend
    property string currentView: "Discover"
    property var items: JSON.parse(backend.rows || "[]")
    property var detail: JSON.parse(backend.details || "{}")
    property bool closePending: false
    property var selected: results.currentIndex >= 0 && results.currentIndex < items.length ? items[results.currentIndex] : null
    property string source: argument("--from", preferences.source)
    property bool useSudo: argument("--auth", preferences.authorization) === "sudo"
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

    globalDrawer: Kirigami.GlobalDrawer {
        title: "PkgDeck"
        titleIcon: "system-software-install"
        isMenu: false
        modal: root.width < 800
        drawerOpen: !modal
        actions: [
            Kirigami.Action {
                text: "Discover"
                icon.name: "applications-all"
                enabled: !backend.busy
                onTriggered: root.openView("Discover")
            },
            Kirigami.Action {
                text: "Search"
                icon.name: "edit-find"
                enabled: !backend.busy
                onTriggered: root.openView("Search")
            },
            Kirigami.Action {
                text: "Installed"
                icon.name: "package-installed-updated"
                enabled: !backend.busy
                onTriggered: root.openView("Installed")
            },
            Kirigami.Action {
                text: "Updates"
                icon.name: "system-software-update"
                enabled: !backend.busy
                onTriggered: root.openView("Updates")
            },
            Kirigami.Action {
                text: "Sources"
                icon.name: "network-server"
                enabled: !backend.busy
                onTriggered: root.openView("Sources")
            },
            Kirigami.Action {
                text: "Settings"
                icon.name: "settings-configure"
                enabled: !backend.busy
                onTriggered: root.openView("Settings")
            },
            Kirigami.Action {
                text: "Help / About"
                icon.name: "help-about"
                enabled: !backend.busy
                onTriggered: root.openView("Help / About")
            }
        ]
    }
    pageStack.initialPage: Kirigami.Page {
        title: root.currentView
        ColumnLayout {
            anchors.fill: parent
            spacing: Kirigami.Units.smallSpacing
            RowLayout {
                Layout.fillWidth: true
                visible: root.currentView === "Search"
                Controls.TextField {
                    id: search
                    objectName: "searchField"
                    Layout.fillWidth: true
                    placeholderText: "Search package names and summaries"
                    Accessible.name: "Search packages"
                    enabled: !backend.busy
                    selectByMouse: true
                    onAccepted: root.reload()
                }
                Controls.Button {
                    text: "Search"
                    enabled: !backend.busy && search.text.trim().length > 0
                    onClicked: root.reload()
                }
            }
            Controls.Label {
                visible: root.currentView === "Discover"
                text: "Explore the package sources available on this computer. Select a source to inspect its capabilities, or use Search to find packages."
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            ColumnLayout {
                visible: root.currentView === "Settings"
                Layout.fillWidth: true
                Controls.Label {
                    text: "Package source"
                }
                Controls.ComboBox {
                    objectName: "sourceSetting"
                    model: ["All available sources", "APT", "Homebrew"]
                    currentIndex: ["", "apt", "homebrew"].indexOf(root.source)
                    onActivated: {
                        root.source = ["", "apt", "homebrew"][currentIndex];
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
                    text: "Polkit uses your host's authentication agent. Sudo requires an existing grant; passwords are never collected here. Homebrew always runs unprivileged. Colors and dark mode follow your system theme."
                }
            }
            Controls.Label {
                visible: root.currentView === "Help / About"
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                text: "PkgDeck 0.1.0\nA unified package interface for Linux.\n\nCtrl+F: search • Ctrl+1: Discover • Ctrl+3: Installed • Ctrl+4: Updates • Ctrl+5: Sources\nCtrl+L: focus results • Up/Down: select • Ctrl+I: install • Ctrl+D: remove • Ctrl+U: upgrade • Ctrl+M: refresh source • Ctrl+R: reload\nEscape: cancel current work\n\nRefresh updates source metadata; Upgrade changes an installed package. Writes require confirmation and may change native dependencies. Cancellation waits for a native write already running.\n\nAPT and Homebrew are supported. Flatpak and Snap host operations remain disabled."
                textFormat: Text.PlainText
            }
            Controls.ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                visible: root.currentView !== "Settings" && root.currentView !== "Help / About"
                ListView {
                    id: results
                    objectName: "packageResults"
                    model: root.items
                    clip: true
                    reuseItems: true
                    currentIndex: -1
                    keyNavigationEnabled: false
                    activeFocusOnTab: true
                    Keys.onDownPressed: root.choose(Math.min(count - 1, currentIndex + 1))
                    Keys.onUpPressed: root.choose(Math.max(0, currentIndex - 1))
                    delegate: Controls.ItemDelegate {
                        id: packageRow
                        required property var modelData
                        required property int index
                        width: ListView.view.width
                        highlighted: results.currentIndex === index
                        enabled: !backend.busy
                        Accessible.name: modelData.name + ", " + modelData.source + ", " + (modelData.summary || "")
                        onClicked: {
                            results.forceActiveFocus();
                            root.choose(index);
                        }
                        contentItem: ColumnLayout {
                            RowLayout {
                                Controls.Label {
                                    color: packageRow.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.textColor
                                    text: modelData.name
                                    textFormat: Text.PlainText
                                    font.bold: true
                                    elide: Text.ElideRight
                                    Layout.fillWidth: true
                                }
                                Controls.Label {
                                    color: packageRow.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.textColor
                                    text: modelData.source
                                    textFormat: Text.PlainText
                                }
                            }
                            Controls.Label {
                                color: packageRow.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.textColor
                                visible: modelData.kind === "package"
                                text: (modelData.installed || "Not installed") + " → " + (modelData.candidate || "Unknown") + "   " + (modelData.architecture || "")
                                textFormat: Text.PlainText
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                            Controls.Label {
                                color: packageRow.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.textColor
                                text: modelData.summary || ""
                                textFormat: Text.PlainText
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }
                        }
                    }
                    Controls.Label {
                        anchors.centerIn: parent
                        width: parent.width - 20
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        visible: results.count === 0 && !backend.busy
                        text: root.currentView === "Search" ? "Enter a search to find packages. No results are currently loaded." : "No results. Check the status below for unavailable sources or errors."
                    }
                }
            }
            Controls.ScrollView {
                visible: root.selected !== null
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(root.height * 0.28, 240)
                Controls.TextArea {
                    objectName: "packageDetails"
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextEdit.Wrap
                    textFormat: TextEdit.PlainText
                    text: root.detail.package ? root.detail.package.name + "\n" + "Scope: " + (root.detail.package.scope_label || "Unknown") + "\n" + (root.detail.description || "") + "\nHomepage: " + (root.detail.homepage || "Unavailable") + "\nDependencies: " + (root.detail.dependencies || []).join(", ") : (root.detail.source || "") + "\n" + (root.detail.availability || "") + "\n" + (root.detail.capabilities || []).join(", ")
                    Accessible.name: "Selected package or source details"
                }
            }
            Flow {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing
                visible: ["Search", "Installed", "Updates", "Sources", "Discover"].indexOf(root.currentView) >= 0
                Controls.Button {
                    objectName: "installButton"
                    text: "Install"
                    enabled: !backend.busy && root.selected !== null && root.selected.kind === "package" && !root.selected.installed
                    onClicked: root.propose("install")
                }
                Controls.Button {
                    objectName: "removeButton"
                    text: "Remove"
                    enabled: !backend.busy && root.selected !== null && !!root.selected.installed
                    onClicked: root.propose("remove")
                }
                Controls.Button {
                    objectName: "upgradeButton"
                    text: "Upgrade"
                    enabled: !backend.busy && root.selected !== null && root.selected.update === "available"
                    onClicked: root.propose("upgrade")
                }
                Controls.Button {
                    objectName: "refreshButton"
                    text: "Refresh source"
                    enabled: !backend.busy && root.selected !== null && root.selected.kind === "source" && root.selected.available
                    onClicked: root.propose("refresh")
                }
                Controls.Button {
                    text: "Reload"
                    enabled: !backend.busy
                    onClicked: root.reload()
                }
            }
            RowLayout {
                Layout.fillWidth: true
                Controls.BusyIndicator {
                    running: backend.busy
                    visible: running
                    Layout.preferredWidth: 32
                    Layout.preferredHeight: 32
                }
                Controls.ScrollView {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 72
                    Controls.TextArea {
                        objectName: "operationStatus"
                        text: backend.status
                        readOnly: true
                        selectByMouse: true
                        textFormat: TextEdit.PlainText
                        wrapMode: TextEdit.Wrap
                        Accessible.name: "Operation status"
                    }
                }
                Controls.Button {
                    text: "Cancel"
                    visible: backend.busy
                    onClicked: backend.cancel()
                }
            }
        }
    }
    Controls.Dialog {
        id: confirmation
        objectName: "confirmationDialog"
        parent: root.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 600)
        height: Math.min(root.height - 32, 420)
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
