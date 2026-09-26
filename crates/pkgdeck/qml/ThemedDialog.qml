import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Controls.Dialog {
    id: dialog
    palette.window: Theme.surface
    palette.base: Theme.surface
    palette.text: Theme.ink
    palette.windowText: Theme.ink
    palette.button: Theme.surface
    palette.buttonText: Theme.ink
    topPadding: 4
    background: Rectangle { color: Theme.surface; radius: Theme.cardRadius + 2; border.color: Theme.line }
    Controls.Overlay.modal: Rectangle {
        color: Qt.rgba(0, 0, 0, Theme.dark ? 0.5 : 0.28)
        Behavior on opacity { NumberAnimation { duration: Theme.revealDuration } }
    }
    enter: Transition {
        ParallelAnimation {
            NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Theme.revealDuration; easing.type: Easing.OutCubic }
            NumberAnimation { property: "scale"; from: Theme.motionEnabled ? 0.96 : 1; to: 1; duration: Theme.revealDuration; easing.type: Easing.OutCubic }
        }
    }
    exit: Transition {
        ParallelAnimation {
            NumberAnimation { property: "opacity"; from: 1; to: 0; duration: Theme.feedbackDuration; easing.type: Easing.InCubic }
            NumberAnimation { property: "scale"; from: 1; to: Theme.motionEnabled ? 0.98 : 1; duration: Theme.feedbackDuration; easing.type: Easing.InCubic }
        }
    }
    header: Item {
        implicitHeight: 58
        Controls.Label {
            anchors.left: parent.left
            anchors.leftMargin: 20
            anchors.right: dialogClose.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            text: dialog.title
            color: Theme.ink
            font.pointSize: Theme.pointSize(1.15)
            font.weight: Font.DemiBold
            elide: Text.ElideRight
        }
        ActionButton {
            id: dialogClose
            anchors.right: parent.right
            anchors.rightMargin: 10
            anchors.verticalCenter: parent.verticalCenter
            text: ""
            symbol: "cancel"
            flat: true
            tooltipText: "Close"
            Accessible.name: "Close dialog"
            onClicked: dialog.reject()
        }
    }
    footer: Controls.DialogButtonBox {
        standardButtons: dialog.standardButtons
        alignment: Qt.AlignRight
        padding: 12
        spacing: 8
        background: Item {}
        delegate: ActionButton { symbol: "" }
        onAccepted: dialog.accept()
        onRejected: dialog.reject()
    }
}
