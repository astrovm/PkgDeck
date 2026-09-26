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
    readonly property string metadataText: detailMatchesSelection ? [
        detailsData.publisher ? "Publisher: " + detailsData.publisher : "",
        detailsData.license ? "License: " + detailsData.license : "",
        detailsData.homepage ? "Homepage: " + detailsData.homepage : "",
        (detailsData.dependencies || []).length ? "Dependencies: " + detailsData.dependencies.join(", ") : ""
    ].filter(Boolean).join("\n") : ""
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
    readonly property real idealHeight: Math.max(88, 32 + 40 + detailsContent.spacing + detailBody.implicitHeight)
    signal closeRequested()
    signal screenshotRequested(string url, string caption)
    signal screenshotFailed(string url, string identity)

    color: surface
    radius: 12
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
                font.pointSize: panel.textFont.pointSize * (panel.compact ? 1.25 : 1.45)
                font.weight: Font.DemiBold
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
                    color: parent.hovered ? Qt.rgba(panel.ink.r, panel.ink.g, panel.ink.b, 0.08) : "transparent"
                    radius: 9
                    border.color: parent.activeFocus ? panel.accent : "transparent"
                    border.width: 2
                    Behavior on color { ColorAnimation { duration: 120 } }
                }
                // A content item fills the button; keep the icon at its size.
                contentItem: Item {
                    DeckIcon { objectName: "closeDetailsIcon"; anchors.centerIn: parent; name: "cancel"; ink: panel.muted; width: 16; height: 16 }
                }
            }
        }
        DeckScrollView {
            ink: panel.muted
            id: detailScroll
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            ColumnLayout {
                id: detailBody
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
                        background: Rectangle { color: panel.canvas; radius: 9; border.color: parent.activeFocus ? panel.accent : panel.line }
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
                    visible: panel.description.length > 0
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
                TextEdit {
                    objectName: "packageMetadata"
                    visible: panel.metadataText.length > 0
                    Layout.fillWidth: true
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextEdit.Wrap
                    textFormat: TextEdit.PlainText
                    color: panel.muted
                    text: panel.metadataText
                    Accessible.name: "Additional package metadata"
                }
            }
        }
    }
}
