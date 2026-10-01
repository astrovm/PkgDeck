import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

// Settings toggle: label on the left, switch on the right. Clicking
// anywhere on the row toggles it.
Controls.Switch {
    id: setting
    // Adds an info icon after the label; hovering it shows this text.
    property string tooltipText: ""
    readonly property alias label: label
    Accessible.description: tooltipText
    Layout.fillWidth: true
    implicitHeight: Math.max(Theme.controlHeight, contentItem.implicitHeight + 8)
    leftPadding: 0
    rightPadding: 0
    indicator: Rectangle {
        x: setting.width - width
        y: (setting.height - height) / 2
        implicitWidth: 38
        implicitHeight: 22
        radius: height / 2
        color: setting.checked ? Theme.accent : Theme.tint(Theme.ink, Theme.dark ? 0.18 : 0.16)
        opacity: setting.enabled ? 1 : 0.45
        Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
        Rectangle {
            width: 16
            height: 16
            radius: 8
            y: 3
            x: setting.checked ? parent.width - width - 3 : 3
            color: setting.checked ? Theme.accentInk : (Theme.dark ? "#e9edf3" : "#ffffff")
            border.color: Theme.tint("#000000", 0.08)
            Behavior on x { NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic } }
        }
        Rectangle {
            anchors.fill: parent
            anchors.margins: -3
            radius: height / 2
            color: "transparent"
            border.color: Theme.accent
            border.width: 2
            visible: setting.visualFocus
        }
    }
    contentItem: RowLayout {
        spacing: 6
        Text {
            id: label
            text: setting.text
            color: setting.enabled ? Theme.ink : Theme.muted
            font: setting.font
            wrapMode: Text.WordWrap
            verticalAlignment: Text.AlignVCenter
            Layout.maximumWidth: setting.availableWidth - 52 - (info.visible ? info.width + 6 : 0)
        }
        DeckIcon {
            id: info
            objectName: "settingInfo"
            name: "info"
            ink: Theme.muted
            visible: setting.tooltipText.length > 0
            Layout.preferredWidth: 16
            Layout.preferredHeight: 16
            HoverHandler { id: infoHover }
            Controls.ToolTip.visible: infoHover.hovered
            Controls.ToolTip.delay: 300
            Controls.ToolTip.text: setting.tooltipText
        }
        // Keeps the label clear of the switch on the right.
        Item { Layout.fillWidth: true; Layout.minimumWidth: 52 }
    }
}
