import QtQuick

// A slim progress line to lay along the bottom of a list row. A value from
// 0 to 1 fills the line; a negative value sweeps a segment instead, or shows
// a still partial segment when motion is off.
Item {
    id: progress
    objectName: "rowProgress"
    // 0..1 for known progress; below 0 for indeterminate.
    property real value: -1
    property bool running: true
    property color color: Theme.accent
    property color trackColor: Theme.tint(color, 0.18)
    readonly property bool indeterminate: value < 0
    readonly property real clampedValue: Math.max(0, Math.min(1, value))
    // True while the sweep is animating; false with motion off.
    readonly property bool sweeping: sweep.running

    implicitHeight: 3
    implicitWidth: 120
    // Fades out when the work ends instead of vanishing mid-frame.
    opacity: running ? 1 : 0
    visible: opacity > 0
    Behavior on opacity {
        enabled: Theme.motionEnabled
        NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic }
    }
    clip: true
    Accessible.role: Accessible.ProgressBar
    Accessible.name: indeterminate ? "Working" : Math.round(clampedValue * 100) + "%"

    Rectangle {
        anchors.fill: parent
        radius: height / 2
        color: progress.trackColor
    }
    Rectangle {
        id: fill
        visible: !progress.indeterminate
        height: parent.height
        width: progress.clampedValue * parent.width
        radius: height / 2
        color: progress.color
        Behavior on width {
            enabled: Theme.motionEnabled
            NumberAnimation { duration: Theme.layoutDuration; easing.type: Easing.OutCubic }
        }
    }
    Rectangle {
        id: segment
        objectName: "rowProgressSegment"
        visible: progress.indeterminate
        height: parent.height
        width: Math.max(24, parent.width * 0.28)
        radius: height / 2
        color: progress.color
        // With motion off the segment rests in the middle, which never reads
        // as a known amount of progress.
        x: (parent.width - width) / 2
        NumberAnimation on x {
            id: sweep
            running: progress.indeterminate && progress.running && progress.visible && Theme.motionEnabled
            from: -segment.width
            to: segment.parent ? segment.parent.width : 0
            duration: Math.max(1, Math.round(Theme.pulseDuration * 1.5))
            loops: Animation.Infinite
            easing.type: Easing.InOutQuad
            onRunningChanged: if (!running) segment.x = Qt.binding(() => (segment.parent.width - segment.width) / 2)
        }
    }
}
