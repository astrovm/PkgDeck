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
    spacing: 4

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
        objectName: "actionProgressBar"
        Layout.fillWidth: true
        from: 0
        to: progress.transferTotal > 0 ? progress.transferTotal : Math.max(1, progress.total)
        value: Math.min(to, progress.transferTotal > 0 ? progress.transferred : progress.done)
        indeterminate: progress.transferTotal <= 0 && progress.total <= 1
        palette.highlight: progress.accent
        Accessible.name: progress.label
    }
}
