import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Rectangle {
    id: panel
    objectName: "detailsPanel"
    property var selected: null
    property string selectionIdentity: ""
    property var screenshots: []
    property string description: ""
    property var detailsData: ({})
    property string iconSource: ""
    property bool compact: false
    property bool motionEnabled: false
    property bool detailMatchesSelection: false
    property color canvas
    property color surface
    property color ink
    property color muted
    property color line
    property color accent
    property font textFont
    property alias detailsContentItem: detailsContent
    signal closeRequested()
    signal screenshotRequested(string url, string caption)
    signal screenshotFailed(string url, string identity)
    signal packageRequested(var packageId)

    color: surface
    radius: 10
    border.color: line
    ColumnLayout {
        id: detailsContent
        objectName: "detailsContent"
        anchors.fill: parent
        anchors.margins: 16
        spacing: 6
        RowLayout {
            Layout.fillWidth: true
            Item {
                visible: detailIcon.status !== Image.Ready
                Layout.preferredWidth: 40
                Layout.preferredHeight: 40
                DeckIcon {
                    name: panel.selected ? (panel.selected.kind === "source" ? panel.selected.source : (panel.selected.kind === "failure" ? "warning" : "package")) : "package"
                    ink: panel.accent
                    anchors.centerIn: parent
                    width: 24
                    height: 24
                }
            }
            Image {
                id: detailIcon
                visible: status === Image.Ready
                asynchronous: true
                source: panel.iconSource
                sourceSize.width: 40
                sourceSize.height: 40
                fillMode: Image.PreserveAspectFit
                Layout.preferredWidth: 40
                Layout.preferredHeight: 40
                Accessible.ignored: true
            }
            Controls.Label {
                text: panel.selected ? (panel.selected.display_name || panel.selected.name) : ""
                textFormat: Text.PlainText
                color: panel.ink
                font.pointSize: panel.textFont.pointSize * (panel.compact ? 1.3 : 1.6)
                font.bold: true
                elide: Text.ElideRight
                Layout.fillWidth: true
            }
            Controls.Button {
                objectName: "closeDetailsButton"
                Accessible.name: "Close details"
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.text: Accessible.name
                Layout.preferredWidth: 38
                implicitHeight: 38
                onClicked: panel.closeRequested()
                background: Rectangle {
                    color: parent.hovered ? panel.canvas : panel.surface
                    radius: 7
                    border.color: parent.activeFocus ? panel.accent : panel.line
                    border.width: parent.activeFocus ? 2 : 1
                }
                contentItem: DeckIcon { name: "cancel"; ink: panel.ink; width: 18; height: 18 }
            }
        }
        Controls.ScrollView {
            id: detailScroll
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            ColumnLayout {
                width: detailScroll.availableWidth
                spacing: 10
                ListView {
                    id: screenshotGallery
                    objectName: "screenshotGallery"
                    visible: panel.screenshots.length > 0
                    Layout.fillWidth: true
                    Layout.preferredHeight: panel.compact ? 72 : 130
                    orientation: ListView.Horizontal
                    spacing: 10
                    clip: true
                    model: panel.screenshots
                    cacheBuffer: 0
                    reuseItems: true
                    delegate: Controls.AbstractButton {
                        required property var modelData
                        width: panel.compact ? 128 : 230
                        height: screenshotGallery.height
                        enabled: preview.status === Image.Ready
                        Accessible.name: modelData.caption || "View app screenshot"
                        Controls.ToolTip.visible: hovered
                        Controls.ToolTip.text: Accessible.name
                        onClicked: panel.screenshotRequested(modelData.url, modelData.caption || "")
                        background: Rectangle { color: panel.canvas; radius: 6; border.color: parent.activeFocus ? panel.accent : panel.line }
                        contentItem: Item {
                            Image {
                                id: preview
                                onStatusChanged: {
                                    if (status === Image.Error)
                                        Qt.callLater(panel.screenshotFailed, modelData.url, panel.selectionIdentity);
                                }
                                anchors.fill: parent
                                source: panel.detailMatchesSelection ? modelData.url : ""
                                asynchronous: true
                                cache: true
                                sourceSize.width: 960
                                sourceSize.height: 540
                                fillMode: Image.PreserveAspectFit
                            }
                            Controls.BusyIndicator {
                                anchors.centerIn: parent
                                width: 28; height: 28
                                visible: preview.status === Image.Loading && panel.motionEnabled
                                running: visible
                            }
                            Controls.Label {
                                anchors.centerIn: parent
                                width: parent.width - 12
                                horizontalAlignment: Text.AlignHCenter
                                color: panel.muted
                                wrapMode: Text.WordWrap
                                text: "Loading…"
                                visible: preview.status === Image.Loading && !panel.motionEnabled
                            }
                        }
                    }
                }
                TextEdit {
                    objectName: "packageDetails"
                    Layout.fillWidth: true
                    readOnly: true
                    selectByMouse: true
                    color: panel.muted
                    padding: 0
                    font.pointSize: panel.textFont.pointSize
                    wrapMode: TextEdit.Wrap
                    textFormat: TextEdit.PlainText
                    text: panel.description
                    Accessible.name: "Package details"
                }
                Controls.Label {
                    visible: panel.detailMatchesSelection
                    text: "Overview"
                    color: panel.ink
                    font.bold: true
                }
                Controls.Label {
                    visible: panel.detailMatchesSelection
                    Layout.fillWidth: true
                    text: panel.selected ? (panel.selected.source + " · " + panel.selected.scope_label
                        + " · " + (panel.selected.installed ? "Installed " + panel.selected.installed
                        : "Available " + (panel.selected.candidate || "version unknown"))) : ""
                    color: panel.muted
                    wrapMode: Text.Wrap
                }
                Controls.Label {
                    visible: panel.detailMatchesSelection
                    text: "Available sources"
                    color: panel.ink
                    font.bold: true
                }
                Repeater {
                    model: panel.detailMatchesSelection ? (panel.detailsData.available_sources || []) : []
                    delegate: Controls.Label {
                        required property var modelData
                        width: detailScroll.availableWidth
                        text: modelData.source + " · " + modelData.name + " · " + modelData.scope_label
                            + " · " + (modelData.candidate || modelData.installed || "version unknown")
                        color: panel.muted
                        wrapMode: Text.WrapAnywhere
                    }
                }
                Controls.Label {
                    visible: panel.detailMatchesSelection
                    Layout.fillWidth: true
                    text: "Variants known from loaded results; search to check more sources."
                    color: panel.muted
                    wrapMode: Text.Wrap
                }
                Controls.Label {
                    visible: panel.detailMatchesSelection
                    text: "Installed copies"
                    color: panel.ink
                    font.bold: true
                }
                Controls.Label {
                    visible: panel.detailMatchesSelection && !(panel.detailsData.installed_copies || []).length
                    text: "No installed copy in loaded results. Open Installed to check all sources."
                    color: panel.muted
                    wrapMode: Text.Wrap
                }
                Repeater {
                    model: panel.detailMatchesSelection ? (panel.detailsData.installed_copies || []) : []
                    delegate: Controls.Button {
                        required property var modelData
                        width: detailScroll.availableWidth
                        text: modelData.source + " · " + modelData.name + " · " + modelData.scope_label
                        Accessible.name: "Open exact installed copy " + text
                        onClicked: panel.packageRequested({backend: modelData.source, name: modelData.name,
                            architecture: modelData.architecture, scope: modelData.scope,
                            remote: modelData.remote, reference: modelData.reference})
                    }
                }
                Controls.Label {
                    visible: panel.detailMatchesSelection && (panel.detailsData.installed_copies || []).length > 0
                    Layout.fillWidth: true
                    text: "Open Installed for a full inventory and audit."
                    color: panel.muted
                    wrapMode: Text.Wrap
                }
                Controls.Label {
                    visible: panel.detailMatchesSelection
                    text: "Technical details"
                    color: panel.ink
                    font.bold: true
                }
                TextEdit {
                    visible: panel.detailMatchesSelection
                    Layout.fillWidth: true
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextEdit.Wrap
                    textFormat: TextEdit.PlainText
                    color: panel.muted
                    text: panel.selected ? [
                        "Identity: " + (panel.selected.reference || panel.selected.name),
                        "Architecture: " + panel.selected.architecture,
                        "Scope: " + panel.selected.scope_label,
                        panel.detailsData.publisher ? "Publisher: " + panel.detailsData.publisher : "",
                        panel.detailsData.license ? "License: " + panel.detailsData.license : "",
                        panel.detailsData.homepage ? "Homepage: " + panel.detailsData.homepage : "",
                        (panel.detailsData.dependencies || []).length ? "Dependencies: " + panel.detailsData.dependencies.join(", ") : ""
                    ].filter(Boolean).join("\n") : ""
                    Accessible.name: "Technical package details"
                }
            }
        }
    }
}
