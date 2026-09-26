import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Controls.TextField {
    id: field
    implicitHeight: Theme.controlHeight + 2
    color: Theme.ink
    placeholderTextColor: Theme.muted
    selectByMouse: true
    leftPadding: 12
    rightPadding: 12
    background: Rectangle {
        color: Theme.surface
        radius: Theme.controlRadius
        border.color: field.activeFocus ? Theme.accent : Theme.line
        border.width: field.activeFocus ? 2 : 1
        Behavior on border.color { ColorAnimation { duration: Theme.feedbackDuration } }
    }
}
