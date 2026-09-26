import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// The history of installs, removals and updates. Works as a page (with its
// own title) or inside a drawer whose shared header replaces the title
// (showHeader: false). Entries with more to tell expand on click, Enter or
// Space to show every change, timings and any log output.
ColumnLayout {
    id: pane
    objectName: "activityPane"
    property var entries: []
    property var progress: ({})
    property bool showHeader: true
    property string title: "Activity"
    // Maps a raw source id (for example "apt") to its display name.
    property var sourceName: (id) => id
    // Shows Cancel on the running entry's progress; emits cancelRunning().
    property bool canCancelRunning: false
    property color surface: Theme.surface
    property color ink: Theme.ink
    property color muted: Theme.muted
    property color line: Theme.line
    property color accent: Theme.accent
    property color danger: Theme.danger
    property color success: Theme.success
    property font textFont: Theme.baseFont
    // Ids of the entries shown expanded; kept here so a model refresh while
    // something runs does not fold them again.
    property var expandedIds: []
    readonly property bool hasQueued: entries.some(entry => entry.state === "queued")
    signal cancelQueued()
    signal cancelRunning()
    spacing: Theme.spacing
    Layout.fillWidth: true
    Layout.fillHeight: true

    function target(operation) {
        operation = operation || {};
        const kind = Object.keys(operation)[0] || "operation";
        const id = operation[kind];
        const action = ({install: "Install", remove: "Remove", upgrade: "Update", upgrade_all: "Update all", refresh: "Refresh", clean: "Clean"})[kind] || "Change";
        if (typeof id !== "object" || id === null)
            return action;
        const name = id.name || id.key || "";
        const backend = id.backend ? String(pane.sourceName(id.backend) || id.backend) : "";
        const scope = id.scope === "system" ? "System" : id.scope && id.scope.user ? "User " + id.scope.user.uid : id.scope && id.scope.environment ? id.scope.environment.path : "";
        const where = [backend, scope].filter(Boolean).join(", ");
        return action + (name ? " " + name : "") + (where ? " (" + where + ")" : "");
    }
    function result(entry) {
        const outcomes = entry.outcomes || [];
        if (!outcomes.length)
            return ({queued: "Queued", running: "In progress", finished: "Completed", failed: "Failed", cancelled: "Cancelled", interrupted: "Interrupted"})[entry.state] || "In progress";
        const done = outcomes.filter(value => value === "finished").length;
        const failed = outcomes.filter(value => value === "failed").length;
        const cancelled = outcomes.filter(value => value === "cancelled").length;
        // A single change reads as its state; a batch counts each outcome.
        if (outcomes.length === 1)
            return failed ? "Failed" : cancelled ? "Cancelled" : "Completed";
        if (!failed && !cancelled)
            return "All " + done + " completed";
        return [done ? done + " completed" : "", failed ? failed + " failed" : "", cancelled ? cancelled + " cancelled" : ""].filter(Boolean).join(", ");
    }
    // Timestamps from today show only the time; older ones add the date.
    function when(seconds) {
        const date = new Date(seconds * 1000);
        const today = new Date();
        const sameDay = date.toDateString() === today.toDateString();
        return sameDay ? date.toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
            : date.toLocaleString(Qt.locale(), Locale.ShortFormat);
    }
    function duration(entry) {
        if (!entry.finished_at || !entry.started_at || entry.finished_at < entry.started_at)
            return "";
        const seconds = entry.finished_at - entry.started_at;
        if (seconds < 60)
            return seconds + " s";
        const minutes = Math.floor(seconds / 60);
        return minutes < 60 ? minutes + " min " + (seconds % 60) + " s" : Math.floor(minutes / 60) + " h " + (minutes % 60) + " min";
    }
    // Log output, when the model carries any.
    function log(entry) {
        const value = entry.log || entry.output || "";
        return Array.isArray(value) ? value.join("\n") : String(value);
    }
    function expandable(entry) {
        return (entry.operations || []).length > 1 || log(entry).length > 0 || !!entry.finished_at;
    }
    function isExpanded(entry) {
        return expandedIds.indexOf(entry.id) >= 0;
    }
    function toggle(entry) {
        const ids = expandedIds.slice();
        const at = ids.indexOf(entry.id);
        if (at >= 0)
            ids.splice(at, 1);
        else
            ids.push(entry.id);
        expandedIds = ids;
    }
    function tone(entry) {
        const outcomes = entry.outcomes || [];
        if (entry.state === "failed" || outcomes.indexOf("failed") >= 0)
            return "failed";
        if (entry.state === "cancelled" || entry.state === "interrupted" || outcomes.indexOf("cancelled") >= 0)
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

    component QuietButton: Controls.Button {
        id: quiet
        Accessible.name: text
        implicitHeight: Theme.controlHeight
        horizontalPadding: Theme.spacingLarge
        background: Rectangle {
            color: quiet.down ? Theme.tint(pane.ink, 0.1) : quiet.hovered ? Theme.hoverTint : pane.surface
            radius: Theme.controlRadius
            // Keyboard focus only; a click leaves no ring.
            border.color: quiet.visualFocus ? pane.accent : pane.line
            border.width: quiet.visualFocus ? 2 : 1
            Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
        }
        contentItem: Text {
            text: quiet.text
            color: pane.ink
            font: pane.textFont
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
    }

    RowLayout {
        objectName: "activityHeader"
        visible: pane.showHeader || pane.hasQueued
        Layout.fillWidth: true
        spacing: Theme.spacing
        Kirigami.Heading {
            objectName: "activityTitle"
            visible: pane.showHeader
            text: pane.title
            elide: Text.ElideRight
            color: pane.ink
            level: 1
            font.pointSize: Theme.pointSize(Theme.headingScale)
            font.bold: true
            Layout.fillWidth: true
        }
        Item { visible: !pane.showHeader; Layout.fillWidth: true }
        QuietButton {
            id: cancelQueued
            objectName: "cancelQueuedButton"
            visible: pane.hasQueued
            text: "Cancel queued"
            onClicked: pane.cancelQueued()
        }
    }
    ListView {
        id: list
        objectName: "activityList"
        Layout.fillWidth: true
        Layout.fillHeight: true
        clip: true
        spacing: Theme.spacingSmall + 2
        keyNavigationEnabled: true
        activeFocusOnTab: count > 0
        model: pane.entries.slice().reverse()
        add: Transition {
            enabled: Theme.motionEnabled
            NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Theme.revealDuration; easing.type: Easing.OutCubic }
            NumberAnimation { property: "y"; from: -8; duration: Theme.revealDuration; easing.type: Easing.OutCubic }
        }
        displaced: Transition {
            enabled: Theme.motionEnabled
            NumberAnimation { property: "y"; duration: Theme.layoutDuration; easing.type: Easing.OutCubic }
        }
        delegate: Rectangle {
            id: entryCard
            required property var modelData
            required property int index
            readonly property color toneColor: pane.toneColor(modelData)
            readonly property bool canExpand: pane.expandable(modelData)
            readonly property bool expanded: canExpand && pane.isExpanded(modelData)
            readonly property bool running: modelData.state === "running" || modelData.state === "authorizing"
            readonly property bool keyboardCurrent: list.activeFocus && ListView.isCurrentItem
            width: ListView.view.width - Theme.scrollGutter
            height: contents.implicitHeight + 24
            color: headerButton.hovered && canExpand ? Qt.tint(pane.surface, Theme.hoverTint) : pane.surface
            radius: Theme.cardRadius
            border.color: keyboardCurrent || headerButton.visualFocus ? pane.accent : pane.line
            border.width: keyboardCurrent || headerButton.visualFocus ? 2 : 1
            clip: true
            Behavior on height {
                enabled: Theme.motionEnabled
                NumberAnimation { duration: Theme.layoutDuration; easing.type: Easing.OutCubic }
            }
            Keys.onReturnPressed: if (canExpand) pane.toggle(modelData)
            Keys.onSpacePressed: if (canExpand) pane.toggle(modelData)
            Accessible.role: Accessible.ListItem
            Accessible.name: pane.target((modelData.operations || [])[0]) + ", " + pane.result(modelData)
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
            // The whole summary line toggles the details.
            Controls.AbstractButton {
                id: headerButton
                objectName: "activityEntryToggle"
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                height: summary.height + 24
                enabled: entryCard.canExpand
                focusPolicy: Qt.NoFocus
                hoverEnabled: true
                Accessible.role: Accessible.Button
                Accessible.name: (entryCard.expanded ? "Hide details for " : "Show details for ") + pane.target((entryCard.modelData.operations || [])[0])
                onClicked: {
                    list.currentIndex = entryCard.index;
                    pane.toggle(entryCard.modelData);
                }
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
                    id: summary
                    Layout.fillWidth: true
                    spacing: Theme.spacing
                    Controls.Label {
                        text: pane.target((modelData.operations || [])[0] || ({}))
                        textFormat: Text.PlainText
                        color: pane.ink
                        font.weight: Font.DemiBold
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                    Controls.Label {
                        objectName: "activityResult"
                        text: pane.result(modelData)
                        color: entryCard.toneColor
                        font.pointSize: Theme.pointSize(Theme.smallScale)
                        font.weight: Font.DemiBold
                        leftPadding: 8
                        rightPadding: 8
                        topPadding: 2
                        bottomPadding: 2
                        background: Rectangle { radius: height / 2; color: Theme.tint(entryCard.toneColor, 0.14) }
                    }
                    Controls.Label {
                        text: pane.when(modelData.started_at)
                        color: pane.muted
                        font.pointSize: Theme.pointSize(Theme.smallScale)
                    }
                    DeckIcon {
                        visible: entryCard.canExpand
                        name: "down"
                        ink: pane.muted
                        rotation: entryCard.expanded ? 180 : 0
                        Layout.preferredWidth: 14
                        Layout.preferredHeight: 14
                        Behavior on rotation {
                            enabled: Theme.motionEnabled
                            NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic }
                        }
                    }
                }
                // Folded batches name how many more changes they hold.
                Controls.Label {
                    visible: !entryCard.expanded && (modelData.operations || []).length > 1
                    text: "and " + ((modelData.operations || []).length - 1) + " more"
                    color: pane.muted
                    font.pointSize: Theme.pointSize(Theme.smallScale)
                }
                ActionProgress {
                    objectName: "activityProgress"
                    readonly property bool active: pane.progress.activity_id === modelData.id
                    visible: entryCard.running
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    label: active ? pane.progress.label : pane.target((modelData.operations || [])[0])
                    done: active ? pane.progress.done || 0 : 0
                    total: active ? pane.progress.total || 1 : 1
                    transferred: active ? pane.progress.transferred || 0 : 0
                    transferTotal: active ? pane.progress.transfer_total || 0 : 0
                    cancelable: pane.canCancelRunning && active
                    onCancelRequested: pane.cancelRunning()
                    ink: pane.ink
                    muted: pane.muted
                    accent: pane.accent
                }
                ColumnLayout {
                    objectName: "activityDetails"
                    visible: entryCard.expanded
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    spacing: 4
                    Repeater {
                        model: entryCard.expanded ? (modelData.operations || []).slice(1) : []
                        delegate: Controls.Label {
                            required property var modelData
                            text: pane.target(modelData)
                            textFormat: Text.PlainText
                            color: pane.ink
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                        }
                    }
                    Controls.Label {
                        visible: text.length > 0
                        text: {
                            const took = pane.duration(modelData);
                            return modelData.finished_at ? "Finished " + pane.when(modelData.finished_at) + (took ? " · took " + took : "") : "";
                        }
                        color: pane.muted
                        font.pointSize: Theme.pointSize(Theme.smallScale)
                    }
                    Rectangle {
                        objectName: "activityLog"
                        visible: logText.text.length > 0
                        Layout.fillWidth: true
                        Layout.preferredHeight: Math.min(logText.implicitHeight + 16, 220)
                        radius: Theme.smallRadius
                        color: Theme.tint(pane.ink, 0.05)
                        border.color: pane.line
                        DeckScrollView {
                            id: logScroll
                            anchors.fill: parent
                            anchors.margins: 8
                            TextEdit {
                                id: logText
                                // Always leaves the scrollbar gutter, so wrapping never
                                // depends on whether the bar shows.
                                width: logScroll.width - Theme.scrollGutter
                                text: entryCard.expanded ? pane.log(modelData) : ""
                                readOnly: true
                                selectByMouse: true
                                wrapMode: TextEdit.WrapAnywhere
                                textFormat: TextEdit.PlainText
                                color: pane.ink
                                font.family: "monospace"
                                font.pointSize: Theme.pointSize(Theme.smallScale)
                                Accessible.name: "Log output"
                            }
                        }
                    }
                }
            }
        }
        Controls.ScrollBar.vertical: DeckScrollBar { ink: pane.muted }
        Column {
            objectName: "activityEmpty"
            anchors.centerIn: parent
            width: Math.min(parent.width - 32, 360)
            spacing: Theme.spacingSmall
            visible: parent.count === 0
            DeckIcon { name: "activity"; ink: pane.muted; width: 34; height: 34; anchors.horizontalCenter: parent.horizontalCenter }
            Item { width: 1; height: 4 }
            Controls.Label {
                objectName: "activityEmptyTitle"
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: "Nothing has run yet"
                color: pane.ink
                font.weight: Font.DemiBold
            }
            Controls.Label {
                objectName: "activityEmptyHint"
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                text: "Installs, removals and updates you run will appear here."
                color: pane.muted
                font.pointSize: Theme.pointSize(Theme.smallScale)
            }
        }
    }
}
