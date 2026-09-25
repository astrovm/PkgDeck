import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

ColumnLayout {
    id: progress
    property string label: ""
    property int done: 0
    property int total: 1
    property real transferred: 0
    property real transferTotal: 0
    property color ink
    property color muted
    property color accent
    spacing: 6

    RowLayout {
        Layout.fillWidth: true
        Controls.Label {
            text: progress.label
            textFormat: Text.PlainText
            color: progress.ink
            elide: Text.ElideRight
            Layout.fillWidth: true
        }
        Controls.Label {
            text: progress.total > 1 ? progress.done + " of " + progress.total : ""
            color: progress.muted
        }
    }
    Controls.ProgressBar {
        id: bar
        objectName: "actionProgressBar"
        Layout.fillWidth: true
        from: 0
        to: progress.transferTotal > 0 ? progress.transferTotal : Math.max(1, progress.total)
        value: Math.min(to, progress.transferTotal > 0 ? progress.transferred : progress.done)
        indeterminate: progress.transferTotal <= 0 && progress.total <= 1
        palette.highlight: progress.accent
        Accessible.name: progress.label
        implicitHeight: 6
        background: Rectangle {
            implicitHeight: 6
            radius: 3
            color: Qt.rgba(progress.ink.r, progress.ink.g, progress.ink.b, 0.1)
        }
        contentItem: Item {
            implicitHeight: 6
            clip: true
            Rectangle {
                visible: !bar.indeterminate
                width: bar.visualPosition * parent.width
                height: parent.height
                radius: 3
                color: progress.accent
                Behavior on width { NumberAnimation { duration: 240; easing.type: Easing.OutCubic } }
            }
            Rectangle {
                id: stripe
                visible: bar.indeterminate
                width: parent.width * 0.3
                height: parent.height
                radius: 3
                color: progress.accent
                NumberAnimation on x {
                    running: bar.indeterminate && bar.visible
                    from: -stripe.width
                    to: stripe.parent.width
                    duration: 1300
                    loops: Animation.Infinite
                    easing.type: Easing.InOutQuad
                }
            }
        }
    }
}
