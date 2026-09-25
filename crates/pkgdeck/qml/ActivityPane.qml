import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

ColumnLayout {
    id: pane
    objectName: "activityPane"
    property var entries: []
    property var progress: ({})
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
        const action = ({install: "Install", remove: "Remove", upgrade: "Update", upgrade_all: "Update all", refresh: "Refresh", clean: "Clean"})[kind] || "Change";
        if (typeof id !== "object" || id === null)
            return action;
        const name = id.name || id.key || "";
        const backend = id.backend || "";
        const scope = id.scope === "system" ? "System" : id.scope && id.scope.user ? "User " + id.scope.user.uid : id.scope && id.scope.environment ? id.scope.environment.path : "";
        return [action + (name ? " " + name : ""), backend, scope].filter(Boolean).join(" · ");
    }
    function result(entry) {
        const outcomes = entry.outcomes || [];
        if (!outcomes.length)
            return ({queued: "Queued", running: "In progress", finished: "Completed", failed: "Failed", cancelled: "Cancelled"})[entry.state] || "In progress";
        const done = outcomes.filter(value => value === "finished").length;
        const failed = outcomes.filter(value => value === "failed").length;
        const cancelled = outcomes.filter(value => value === "cancelled").length;
        return done + " finished" + (failed ? " · " + failed + " failed" : "") + (cancelled ? " · " + cancelled + " cancelled" : "");
    }
    // Timestamps from today show only the time; older ones add the date.
    function when(seconds) {
        const date = new Date(seconds * 1000);
        const today = new Date();
        const sameDay = date.toDateString() === today.toDateString();
        return sameDay ? date.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
            : date.toLocaleString(Qt.locale(), Locale.ShortFormat);
    }
    function tone(entry) {
        const outcomes = entry.outcomes || [];
        if (entry.state === "failed" || outcomes.indexOf("failed") >= 0)
            return "failed";
        if (entry.state === "cancelled" || outcomes.indexOf("cancelled") >= 0)
            return "cancelled";
        if (entry.state === "queued")
            return "queued";
        if (entry.state === "running" || entry.state === "authorizing")
            return "running";
        return "finished";
    }
    function toneColor(entry) {
        return ({failed: pane.danger, cancelled: pane.muted, queued: pane.muted, running: pane.accent, finished: pane.success})[tone(entry)];
    }
    property color danger: "#e5484d"
    property color success: "#30a46c"
    RowLayout {
        visible: pane.entries.some(entry => entry.state === "queued")
        Layout.fillWidth: true
        Item { Layout.fillWidth: true }
        Controls.Button {
            id: cancelQueued
            objectName: "cancelQueuedButton"
            text: "Cancel queued"
            onClicked: pane.cancelQueued()
            Accessible.name: text
            implicitHeight: Math.max(36, Math.round(pane.textFont.pointSize * 2.9))
            horizontalPadding: 16
            background: Rectangle {
                color: cancelQueued.hovered ? Qt.rgba(pane.ink.r, pane.ink.g, pane.ink.b, 0.06) : pane.surface
                radius: 9
                border.color: cancelQueued.activeFocus ? pane.accent : pane.line
                border.width: cancelQueued.activeFocus ? 2 : 1
            }
            contentItem: Text {
                text: cancelQueued.text
                color: pane.ink
                font: pane.textFont
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
            }
        }
    }
    ListView {
        objectName: "activityList"
        Layout.fillWidth: true
        Layout.fillHeight: true
        clip: true
        spacing: 8
        model: pane.entries.slice().reverse()
        delegate: Rectangle {
            id: entryCard
            required property var modelData
            readonly property color toneColor: pane.toneColor(modelData)
            width: ListView.view.width - 12
            height: contents.implicitHeight + 24
            color: pane.surface
            radius: 12
            border.color: pane.line
            Rectangle {
                width: 3
                radius: 1.5
                anchors.left: parent.left
                anchors.leftMargin: 6
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                anchors.margins: 12
                color: entryCard.toneColor
            }
            ColumnLayout {
                id: contents
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.leftMargin: 20
                anchors.rightMargin: 14
                anchors.topMargin: 12
                spacing: 4
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 10
                    Controls.Label {
                        text: pane.target((modelData.operations || [])[0] || ({}))
                        textFormat: Text.PlainText
                        color: pane.ink
                        font.weight: Font.DemiBold
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Controls.Label {
                        text: pane.result(modelData)
                        color: entryCard.toneColor
                        font.pointSize: pane.textFont.pointSize * 0.88
                        font.weight: Font.DemiBold
                        leftPadding: 8
                        rightPadding: 8
                        topPadding: 2
                        bottomPadding: 2
                        background: Rectangle { radius: height / 2; color: Qt.rgba(entryCard.toneColor.r, entryCard.toneColor.g, entryCard.toneColor.b, 0.14) }
                    }
                    Controls.Label {
                        text: pane.when(modelData.started_at)
                        color: pane.muted
                        font.pointSize: pane.textFont.pointSize * 0.88
                    }
                }
                Repeater {
                    model: (modelData.operations || []).slice(1)
                    delegate: Controls.Label {
                        required property var modelData
                        text: pane.target(modelData)
                        textFormat: Text.PlainText
                        color: pane.muted
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                }
                ActionProgress {
                    objectName: "activityProgress"
                    readonly property bool active: pane.progress.activity_id === modelData.id
                    visible: modelData.state === "running" || modelData.state === "authorizing"
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    label: active ? pane.progress.label : pane.target((modelData.operations || [])[0])
                    done: active ? pane.progress.done || 0 : 0
                    total: active ? pane.progress.total || 1 : 1
                    transferred: active ? pane.progress.transferred || 0 : 0
                    transferTotal: active ? pane.progress.transfer_total || 0 : 0
                    ink: pane.ink
                    muted: pane.muted
                    accent: pane.accent
                }
            }
        }
        Controls.ScrollBar.vertical: Controls.ScrollBar { }
        Column {
            anchors.centerIn: parent
            spacing: 10
            visible: parent.count === 0
            DeckIcon { name: "activity"; ink: pane.muted; width: 34; height: 34; anchors.horizontalCenter: parent.horizontalCenter }
            Controls.Label {
                text: "No operations yet"
                color: pane.muted
                anchors.horizontalCenter: parent.horizontalCenter
            }
        }
    }
}
