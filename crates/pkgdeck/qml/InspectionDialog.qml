import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Controls.Dialog {
    id: panel
    objectName: "inspectionDialog"
    required property var backend
    property var reportData: ({})
    property string mode: "command"
    property color ink
    property color muted
    property color surface
    property color line
    signal exactPackage(var packageId)
    function scopeText(scope) {
        if (scope === "system") return "System";
        if (scope && scope.user) return "User " + scope.user.uid;
        if (scope && scope.environment) return scope.environment.path;
        return "Scope unknown";
    }

    title: mode === "command" ? "Inspect command" : "Installed copies and data"
    modal: true
    standardButtons: Controls.Dialog.Close
    contentItem: ColumnLayout {
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            Controls.Button {
                objectName: "commandInspectionTab"
                text: "Command"
                checkable: true
                checked: panel.mode === "command"
                onClicked: panel.mode = "command"
            }
            Controls.Button {
                objectName: "installedAuditTab"
                text: "Installed audit"
                checkable: true
                checked: panel.mode === "audit"
                onClicked: { panel.mode = "audit"; panel.backend.auditInstalled(); }
            }
            Item { Layout.fillWidth: true }
        }
        RowLayout {
            Layout.fillWidth: true
            visible: panel.mode === "command"
            Controls.TextField {
                id: commandField
                objectName: "inspectCommandField"
                Layout.fillWidth: true
                placeholderText: "Command name, for example git"
                Accessible.name: "Command to inspect"
                selectByMouse: true
                onAccepted: if (text.trim()) panel.backend.inspectCommand(text.trim())
            }
            Controls.Button {
                objectName: "runCommandInspection"
                text: "Inspect"
                enabled: commandField.text.trim().length > 0 && !panel.backend.busy
                onClicked: panel.backend.inspectCommand(commandField.text.trim())
            }
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: panel.backend.busy
            text: "Reading installed sources…"
            color: panel.muted
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: panel.reportData.kind === "error"
            text: panel.reportData.message || "Inspection failed."
            color: panel.ink
            wrapMode: Text.Wrap
        }
        Controls.Label {
            Layout.fillWidth: true
            visible: (panel.reportData.failures || []).length > 0
            text: (panel.reportData.failures || []).length + " sources could not report installed packages; ownership and duplicate results may be incomplete."
            color: panel.muted
            wrapMode: Text.Wrap
        }
        Controls.ScrollView {
            id: reportScroll
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            Column {
                width: reportScroll.availableWidth
                spacing: 12
                visible: panel.reportData.kind === panel.mode
                Controls.Label {
                    width: parent.width
                    visible: panel.mode === "command"
                    text: "Resolved: " + ((panel.reportData.report || {}).resolved || "No executable found")
                    color: panel.ink
                    font.bold: true
                    wrapMode: Text.Wrap
                }
                Controls.Label {
                    width: parent.width
                    visible: panel.mode === "command"
                    text: "PATH order · " + ((panel.reportData.report || {}).environment || "host environment")
                    color: panel.muted
                    wrapMode: Text.Wrap
                }
                Controls.Label {
                    width: parent.width
                    visible: panel.mode === "command"
                    text: "PATH: " + ((panel.reportData.report || {}).path || "")
                    color: panel.muted
                    wrapMode: Text.WrapAnywhere
                }
                Repeater {
                    model: panel.mode === "command" ? ((panel.reportData.report || {}).candidates || []) : []
                    delegate: Rectangle {
                        id: candidateCard
                        required property var modelData
                        width: reportScroll.availableWidth
                        implicitHeight: candidateContent.implicitHeight + 20
                        color: panel.surface
                        radius: 8
                        border.color: panel.line
                        Column {
                            id: candidateContent
                            anchors.fill: parent
                            anchors.margins: 10
                            spacing: 4
                            Controls.Label {
                                width: candidateContent.width
                                color: panel.ink
                                wrapMode: Text.WrapAnywhere
                                text: candidateCard.modelData.path + " · " + candidateCard.modelData.state
                                    + (candidateCard.modelData.target ? "\n→ " + candidateCard.modelData.target : "")
                            }
                            Repeater {
                                model: candidateCard.modelData.owners || []
                                delegate: Column {
                                    id: ownerRow
                                    required property var modelData
                                    width: candidateContent.width
                                    Controls.Label {
                                        width: parent.width
                                        color: panel.muted
                                        text: ownerRow.modelData.state === "unknown" ? "Owner unknown" :
                                            ownerRow.modelData.manager + ": " + ownerRow.modelData.native_name
                                                + " (" + ownerRow.modelData.state + ") · " + ownerRow.modelData.path
                                        wrapMode: Text.Wrap
                                    }
                                    Repeater {
                                        model: ownerRow.modelData.packages || []
                                        delegate: Controls.Button {
                                            required property var modelData
                                            width: candidateContent.width
                                            text: "Open " + modelData.backend + " · " + modelData.name
                                            Accessible.name: "Open exact installed copy " + text
                                            onClicked: panel.exactPackage(modelData)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Controls.Label {
                    width: parent.width
                    visible: panel.mode === "audit"
                    text: ((panel.reportData.report || {}).groups || []).length + " known duplicate groups"
                    color: panel.ink
                    font.bold: true
                }
                Repeater {
                    model: panel.mode === "audit" ? ((panel.reportData.report || {}).groups || []) : []
                    delegate: Rectangle {
                        id: groupCard
                        required property var modelData
                        width: reportScroll.availableWidth
                        implicitHeight: groupContent.implicitHeight + 20
                        color: panel.surface
                        radius: 8
                        border.color: panel.line
                        Column {
                            id: groupContent
                            anchors.fill: parent
                            anchors.margins: 10
                            spacing: 3
                            Controls.Label { text: groupCard.modelData.copies.length ? (groupCard.modelData.copies[0].display_name || groupCard.modelData.key) : groupCard.modelData.key; color: panel.ink; font.bold: true }
                            Repeater {
                                model: groupCard.modelData.copies || []
                                delegate: Controls.Button {
                                    required property var modelData
                                    width: groupContent.width
                                    text: modelData.package.backend + " · " + modelData.package.name
                                        + " · " + panel.scopeText(modelData.package.scope)
                                        + " · " + modelData.installed_version
                                        + (modelData.location ? "\n" + modelData.location : "")
                                    Accessible.name: "Open exact installed copy " + text
                                    onClicked: panel.exactPackage(modelData.package)
                                }
                            }
                        }
                    }
                }
                Controls.Label {
                    width: parent.width
                    visible: panel.mode === "audit"
                    text: ((panel.reportData.report || {}).installed_copies || []).length
                        + " exact installed copies"
                    color: panel.muted
                    wrapMode: Text.Wrap
                }
                Repeater {
                    model: panel.mode === "audit" ? ((panel.reportData.report || {}).installed_copies || []) : []
                    delegate: Controls.Button {
                        required property var modelData
                        width: reportScroll.availableWidth
                        text: modelData.package.backend + " · " + modelData.package.name
                            + " · " + panel.scopeText(modelData.package.scope)
                            + " · " + modelData.installed_version
                            + (modelData.location ? "\n" + modelData.location : "")
                        Accessible.name: "Open exact installed copy " + text
                        onClicked: panel.exactPackage(modelData.package)
                    }
                }
                Controls.Label {
                    width: parent.width
                    visible: panel.mode === "audit"
                    text: ((panel.reportData.report || {}).leftovers || []).length + " manager-reported residual files"
                    color: panel.ink
                    font.bold: true
                }
                Repeater {
                    model: panel.mode === "audit" ? ((panel.reportData.report || {}).leftovers || []) : []
                    delegate: Controls.Label {
                        required property var modelData
                        width: reportScroll.availableWidth
                        text: modelData.manager + " · " + modelData.native_name + "\n" + modelData.path
                            + (modelData.size_bytes === null ? "" : " · " + modelData.size_bytes + " bytes")
                        color: panel.muted
                        wrapMode: Text.WrapAnywhere
                    }
                }
                Controls.Label {
                    width: parent.width
                    visible: panel.reportData.kind === panel.mode
                    text: (panel.reportData.report || {}).data_note || (panel.reportData.report || {}).ownership_note || ""
                    color: panel.muted
                    wrapMode: Text.Wrap
                }
            }
        }
    }
}
