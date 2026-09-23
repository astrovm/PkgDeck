import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

ColumnLayout {
    id: pane
    objectName: "activityPane"
    property var entries: []
    property var backgroundState: ({})
    property color surface
    property color ink
    property color muted
    property color line
    property color accent
    property font textFont
    signal cancelQueued()
    spacing: 8
    Layout.fillWidth: true
    Layout.fillHeight: true

    function target(operation) {
        const kind = Object.keys(operation || {})[0] || "operation";
        const id = operation[kind];
        if (typeof id !== "object" || id === null)
            return kind;
        const scope = id.scope === "system" ? "System" : id.scope && id.scope.user ? "User " + id.scope.user.uid : id.scope && id.scope.environment ? id.scope.environment.path : "";
        return kind.replace(/_/g, " ") + " · " + (id.name || id.key || id.backend || "") + " · " + (id.backend || "") + (scope ? " · " + scope : "");
    }
    function result(entry) {
        const outcomes = entry.outcomes || [];
        if (!outcomes.length)
            return entry.state;
        const done = outcomes.filter(value => value === "finished").length;
        const failed = outcomes.filter(value => value === "failed").length;
        const cancelled = outcomes.filter(value => value === "cancelled").length;
        return done + " finished" + (failed ? " · " + failed + " failed" : "") + (cancelled ? " · " + cancelled + " cancelled" : "");
    }
    RowLayout {
        Layout.fillWidth: true
        Controls.Label { text: "Recent operations"; font.bold: true; color: pane.ink; Layout.fillWidth: true }
        Controls.Button {
            objectName: "cancelQueuedButton"
            text: "Cancel queued"
            enabled: pane.entries.some(entry => entry.state === "queued")
            onClicked: pane.cancelQueued()
            Accessible.name: text
        }
    }
    Controls.Label {
        Layout.fillWidth: true
        visible: !!pane.backgroundState.last_check
        color: pane.muted
        text: pane.backgroundState.last_check ? "Updates checked " + new Date(pane.backgroundState.last_check * 1000).toLocaleString() + " · " + pane.backgroundState.available + " available" + (pane.backgroundState.failures && pane.backgroundState.failures.length ? " · " + pane.backgroundState.failures.length + " sources failed" : "") : ""
        elide: Text.ElideRight
    }
    ListView {
        objectName: "activityList"
        Layout.fillWidth: true
        Layout.fillHeight: true
        clip: true
        spacing: 6
        model: pane.entries.slice().reverse()
        delegate: Rectangle {
            required property var modelData
            width: ListView.view.width
            height: contents.implicitHeight + 20
            color: pane.surface
            radius: 8
            border.color: pane.line
            ColumnLayout {
                id: contents
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.margins: 10
                spacing: 4
                RowLayout {
                    Layout.fillWidth: true
                    Controls.Label { text: pane.result(modelData); color: pane.accent; font.bold: true; Layout.fillWidth: true }
                    Controls.Label { text: new Date(modelData.started_at * 1000).toLocaleString(); color: pane.muted }
                }
                Repeater {
                    model: modelData.operations || []
                    delegate: Controls.Label {
                        required property var modelData
                        text: pane.target(modelData)
                        textFormat: Text.PlainText
                        color: pane.ink
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                }
            }
        }
        Controls.ScrollBar.vertical: Controls.ScrollBar { }
        Controls.Label {
            anchors.centerIn: parent
            text: "No operations yet"
            color: pane.muted
            visible: parent.count === 0
        }
    }
}
