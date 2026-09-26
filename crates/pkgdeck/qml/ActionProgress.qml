import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

// One line of progress: what is running, a bar filling the rest of the
// width, the step count or percentage, and an optional Cancel button.
// Narrow widths stack the label above the bar.
GridLayout {
    id: progress
    property string label: ""
    property int done: 0
    property int total: 1
    property real transferred: 0
    property real transferTotal: 0
    property color ink: Theme.ink
    property color muted: Theme.muted
    property color accent: Theme.accent
    // Shows a Cancel button that emits cancelRequested().
    property bool cancelable: false
    property string cancelText: "Cancel"
    // Below this width the label sits above the bar.
    property real stackWidth: 460
    readonly property bool stacked: width > 0 && width < stackWidth
    readonly property bool indeterminate: bar.indeterminate
    readonly property string countText: transferTotal > 0
        ? Math.round(100 * Math.min(1, transferred / transferTotal)) + "%"
        : total > 1 ? done + " of " + total : ""
    signal cancelRequested()

    // Every cell is placed explicitly. Wide: label, bar, count, Cancel.
    // Stacked: label, count, Cancel, with the bar on its own row below.
    columnSpacing: Theme.spacing
    rowSpacing: Theme.spacingSmall

    Controls.Label {
        objectName: "actionProgressLabel"
        text: progress.label
        textFormat: Text.PlainText
        color: progress.ink
        elide: Text.ElideRight
        maximumLineCount: 1
        Layout.row: 0
        Layout.column: 0
        Layout.fillWidth: progress.stacked
        // Wide: the label takes what it needs, up to a share of the line.
        Layout.maximumWidth: progress.stacked ? -1 : Math.max(120, progress.width * 0.45)
    }
    Controls.ProgressBar {
        id: bar
        objectName: "actionProgressBar"
        Layout.fillWidth: true
        Layout.row: progress.stacked ? 1 : 0
        Layout.column: progress.stacked ? 0 : 1
        Layout.columnSpan: progress.stacked ? 3 : 1
        Layout.alignment: Qt.AlignVCenter
        from: 0
        to: progress.transferTotal > 0 ? progress.transferTotal : Math.max(1, progress.total)
        value: Math.min(to, progress.transferTotal > 0 ? progress.transferred : progress.done)
        indeterminate: progress.transferTotal <= 0 && progress.total <= 1
        palette.highlight: progress.accent
        Accessible.name: progress.label
        implicitHeight: 6
        implicitWidth: 120
        background: Rectangle {
            implicitHeight: 6
            radius: 3
            color: Theme.tint(progress.ink, 0.1)
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
                Behavior on width {
                    enabled: Theme.motionEnabled
                    NumberAnimation { duration: Theme.layoutDuration; easing.type: Easing.OutCubic }
                }
            }
            // Motion off: a still, striped bar says "working" without
            // pretending to know how far along it is.
            Rectangle {
                objectName: "actionProgressStripes"
                visible: bar.indeterminate && !Theme.motionEnabled
                anchors.fill: parent
                radius: 3
                color: Theme.tint(progress.accent, 0.35)
                clip: true
                Row {
                    x: -8
                    spacing: 6
                    Repeater {
                        model: Math.ceil((bar.width + 16) / 10)
                        Rectangle {
                            width: 4
                            height: 14
                            y: -4
                            rotation: 30
                            color: progress.accent
                            opacity: 0.6
                        }
                    }
                }
            }
            Rectangle {
                id: stripe
                objectName: "actionProgressSweep"
                visible: bar.indeterminate && Theme.motionEnabled
                width: parent.width * 0.3
                height: parent.height
                radius: 3
                color: progress.accent
                NumberAnimation on x {
                    running: stripe.visible && bar.visible
                    from: -stripe.width
                    to: stripe.parent.width
                    duration: Math.max(1, Math.round(Theme.pulseDuration * 1.6))
                    loops: Animation.Infinite
                    easing.type: Easing.InOutQuad
                }
            }
        }
    }
    Controls.Label {
        objectName: "actionProgressCount"
        visible: text.length > 0
        text: progress.countText
        color: progress.muted
        font.pointSize: Theme.pointSize(Theme.smallScale)
        font.features: ({"tnum": 1})
        Layout.row: 0
        Layout.column: progress.stacked ? 1 : 2
        Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
    }
    Controls.Button {
        id: cancelButton
        objectName: "actionProgressCancel"
        visible: progress.cancelable
        text: progress.cancelText
        Accessible.name: progress.cancelText + " " + progress.label
        Layout.row: 0
        Layout.column: progress.stacked ? 2 : 3
        Layout.alignment: Qt.AlignVCenter
        implicitHeight: Math.round(Theme.controlHeight * 0.8)
        horizontalPadding: Theme.spacing
        onClicked: progress.cancelRequested()
        background: Rectangle {
            radius: Theme.controlRadius
            color: cancelButton.down ? Theme.tint(progress.ink, 0.1) : cancelButton.hovered ? Theme.hoverTint : "transparent"
            border.color: cancelButton.visualFocus ? progress.accent : Theme.line
            border.width: cancelButton.visualFocus ? 2 : 1
            Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
        }
        contentItem: Text {
            text: cancelButton.text
            color: progress.ink
            font.pointSize: Theme.pointSize(Theme.smallScale)
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
    }
}
