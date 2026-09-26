import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Controls.ComboBox {
    id: combo
    implicitHeight: Theme.controlHeight
    indicator: DeckIcon {
        x: combo.width - width - 12
        y: (combo.height - height) / 2
        width: 14
        height: 14
        name: "down"
        ink: Theme.muted
        rotation: combo.popup.visible ? 180 : 0
        Behavior on rotation { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic } }
    }
    background: Rectangle {
        color: Theme.surface
        radius: Theme.controlRadius
        border.color: combo.visualFocus ? Theme.accent : Theme.line
        border.width: combo.visualFocus ? 2 : 1
        Rectangle {
            anchors.fill: parent
            radius: parent.radius
            color: combo.hovered ? Theme.hoverTint : "transparent"
            Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
        }
    }
    contentItem: Controls.TextField {
        text: combo.displayText
        color: combo.enabled ? Theme.ink : Theme.muted
        enabled: false
        readOnly: true
        selectByMouse: false
        background: null
        clip: true
        verticalAlignment: Text.AlignVCenter
        leftPadding: 14
        rightPadding: 34
    }
    delegate: Controls.ItemDelegate {
        required property var modelData
        required property int index
        width: ListView.view ? ListView.view.width : combo.width
        text: modelData
        font: combo.font
        highlighted: combo.highlightedIndex === index
        background: Rectangle {
            radius: Theme.controlRadius - 3
            color: highlighted ? Theme.selection : "transparent"
            Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
        }
        contentItem: RowLayout {
            spacing: 8
            Text {
                text: modelData
                color: Theme.ink
                font: combo.font
                elide: Text.ElideRight
                verticalAlignment: Text.AlignVCenter
                leftPadding: 6
                Layout.fillWidth: true
            }
            DeckIcon {
                name: "installed"
                ink: Theme.accent
                visible: combo.currentIndex === index
                Layout.preferredWidth: 14
                Layout.preferredHeight: 14
            }
        }
    }
    popup: Controls.Popup {
        y: combo.height + 4
        width: combo.width
        implicitHeight: Math.min(contentItem.implicitHeight + topPadding + bottomPadding, 320)
        padding: 5
        enter: Transition {
            ParallelAnimation {
                NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Theme.feedbackDuration; easing.type: Easing.OutCubic }
                NumberAnimation { property: "scale"; from: Theme.motionEnabled ? 0.97 : 1; to: 1; duration: Theme.feedbackDuration; easing.type: Easing.OutCubic }
            }
        }
        exit: Transition {
            NumberAnimation { property: "opacity"; from: 1; to: 0; duration: Theme.feedbackDuration; easing.type: Easing.InCubic }
        }
        contentItem: ListView {
            clip: true
            implicitHeight: contentHeight
            model: combo.popup.visible ? combo.delegateModel : null
            currentIndex: combo.highlightedIndex
            delegate: combo.delegate
            Controls.ScrollBar.vertical: DeckScrollBar { ink: Theme.muted }
        }
        background: Rectangle {
            color: Theme.surface
            radius: Theme.controlRadius + 2
            border.color: Theme.line
        }
    }
}
