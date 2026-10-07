import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

// Details for the selected row. The header (icon, name, source, installed
// state, the row's action and Close) follows the selection at once; the
// body shows a skeleton until the details for that same row arrive.
// As a page (`page`), it is a package's store page: Back, a large icon,
// the version, big screenshots, and changes to review before they run.
Rectangle {
    id: panel
    objectName: "detailsPanel"
    property var selected: null
    property string selectionIdentity: ""
    property var screenshots: []
    property string description: ""
    property var detailsData: ({})
    // Plain-text summary of the facts below; kept for callers that test
    // whether there is anything beyond the description.
    readonly property string metadataText: detailMatchesSelection ? [
        detailsData.publisher ? "Publisher: " + detailsData.publisher : "",
        detailsData.license ? "License: " + detailsData.license : "",
        detailsData.homepage ? "Homepage: " + detailsData.homepage : "",
        (detailsData.dependencies || []).length ? "Dependencies: " + detailsData.dependencies.join(", ") : "",
        location ? "File: " + location : ""
    ].filter(Boolean).join("\n") : ""
    property string iconSource: ""
    property bool page: false
    property bool embedded: false
    property var installationChoices: []
    property int installationIndex: 0
    // Changes to look at before they run, shown on the page: the summary's
    // first line names them, the rest and `reviewDetails` explain them.
    property string reviewSummary: ""
    property string reviewDetails: ""
    property string reviewActionText: ""
    property bool reviewDanger: false
    // Starts the installed app; empty text hides the button.
    property string launchText: ""
    // How an installed AppImage starts ({arguments, environment, editable}),
    // shown for editing on its page when PkgDeck manages it.
    property var launchSettings: null
    property string launchError: ""
    // Where an AppImage PkgDeck manages updates from ({github, builtin,
    // editable}), set on its page.
    property var updateSource: null
    property string updateError: ""
    // The file an installed AppImage starts from ({path, folder, bytes,
    // modified, managed, without_fuse}), on its page.
    property var appFile: null
    // How it gets updates, in a few words (see Browser's updateState).
    property string updateState: ""
    // One line under the header that explains the page's own action.
    property string note: ""
    property bool compact: false
    property bool motionEnabled: Theme.motionEnabled
    property bool detailMatchesSelection: false
    // The row's own action, repeated in the header. Empty text hides it.
    property string actionText: ""
    property string actionSymbol: "install"       // DeckIcon: install | remove | updates
    property string actionTone: "accent"          // success | danger | accent
    property bool actionEnabled: true
    // Spoken name for the action; empty means the action text plus the package.
    property string actionAccessibleName: ""
    // An optional second action beside the main one (e.g. Manage next to
    // Remove for an AppImage installed some other way).
    property string secondaryActionText: ""
    property string secondaryActionSymbol: "install"
    property string secondaryActionTone: "accent"
    // An app you can start opens with Launch as its main button, unless an
    // update waits; removal stays beside it as a trash icon.
    readonly property bool launchIsMain: page && launchText.length > 0 && actionSymbol !== "updates"
    // Maps a raw source id to its display name for the header.
    property var sourceName: (id) => id
    property color canvas: Theme.canvas
    property color surface: Theme.surface
    property color ink: Theme.ink
    property color muted: Theme.muted
    property color line: Theme.line
    property color accent: Theme.accent
    property font textFont: Theme.baseFont
    property alias detailsContentItem: detailsContent
    // A package whose details have not arrived yet shows the skeleton.
    readonly property bool loading: selected !== null && selected !== undefined
        && selected.kind === "package" && !detailMatchesSelection
    // Anything to show beyond the header, once details are in.
    readonly property bool hasDetails: detailMatchesSelection
        && (description.length > 0 || screenshots.length > 0 || metadataText.length > 0)
    // A first look, such as what the row already says, while the rest is
    // still on its way (such as Flathub's description and screenshots).
    readonly property bool loadingMore: detailMatchesSelection && detailsData.more === true
    readonly property real contentIdealHeight: Math.max(88, 2 * Theme.gutter + header.implicitHeight
        + detailsContent.spacing + detailBody.implicitHeight)
    // Never collapses while the next row loads: it keeps the last settled
    // height (or the skeleton's, if taller) so the panel does not jump.
    readonly property real idealHeight: loading ? Math.max(settledHeight, contentIdealHeight) : contentIdealHeight
    property real settledHeight: 0
    property bool dependenciesExpanded: false
    readonly property int thumbnailWidth: page && !embedded ? 360 : 230
    readonly property int thumbnailHeight: page && !embedded ? 225 : 130
    readonly property int iconSize: page && !embedded ? 72 : 44
    // The version a page shows: the one installed, else the one on offer.
    readonly property string version: selected && selected.kind === "package"
        ? String(selected.installed || selected.candidate || "") : ""
    // Where an opened file or link came from.
    readonly property string location: appFile && appFile.path ? String(appFile.path)
        : detailMatchesSelection && detailsData.location ? String(detailsData.location) : ""
    function sizeText(bytes) {
        if (!(bytes > 0))
            return "";
        const units = ["bytes", "KB", "MB", "GB"];
        let value = bytes, unit = 0;
        while (value >= 1000 && unit < units.length - 1) {
            value /= 1000;
            unit++;
        }
        return (unit === 0 ? value : value.toFixed(value < 10 ? 1 : 0)) + " " + units[unit];
    }
    // The one-line summary, above the long description on a page.
    readonly property string summary: page && selected && selected.kind === "package" && selected.summary
        && String(selected.summary).trim() !== description.trim() ? String(selected.summary) : ""
    signal closeRequested()
    signal reviewAccepted()
    signal reviewRejected()
    signal launchRequested()
    signal launchSettingsSaved(string arguments, var environment)
    signal updateSourceSaved(string github)
    signal screenshotRequested(string url, string caption)
    signal screenshotFailed(string url, string identity)
    signal actionRequested()
    signal secondaryActionRequested()
    signal showInFolderRequested(string folder)
    signal installationRequested(int index)

    onContentIdealHeightChanged: if (!loading && selected) settledHeight = contentIdealHeight
    onLoadingChanged: if (!loading && selected) settledHeight = contentIdealHeight
    onSelectedChanged: if (!selected) settledHeight = 0
    onSelectionIdentityChanged: dependenciesExpanded = false

    readonly property color actionColor: toneColor(actionTone)
    readonly property color secondaryActionColor: toneColor(secondaryActionTone)
    function toneColor(tone) {
        return tone === "success" ? Theme.success : tone === "danger" ? Theme.danger : accent;
    }
    component DetailsAction: Controls.Button {
        id: chip
        property string label: ""
        property string symbol: "install"
        property color tone: panel.accent
        property string accessibleName: ""
        // Just the icon; the label is still its spoken name and tooltip.
        property bool iconOnly: false
        // The page's main action: filled, so it reads first.
        property bool primary: false
        visible: label.length > 0
        text: panel.compact || iconOnly ? "" : label
        Accessible.name: accessibleName || (label + (panel.selected ? " " + (panel.selected.display_name || panel.selected.name || "") : ""))
        Controls.ToolTip.visible: hovered && (panel.compact || iconOnly)
        Controls.ToolTip.delay: 500
        Controls.ToolTip.text: label
        implicitHeight: Theme.controlHeight
        horizontalPadding: panel.compact ? 9 : 14
        Layout.alignment: Qt.AlignVCenter
        scale: down && panel.motionEnabled ? 0.97 : 1
        Behavior on scale { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic } }
        background: Rectangle {
            radius: Theme.controlRadius
            color: !chip.enabled ? "transparent"
                : chip.primary ? Qt.darker(chip.tone, chip.down ? 1.2 : chip.hovered ? 1.08 : 1)
                : Theme.tint(chip.tone, chip.down ? 0.26 : chip.hovered ? 0.2 : 0.13)
            border.color: chip.enabled ? (chip.primary ? chip.tone : Theme.tint(chip.tone, 0.4)) : panel.line
            Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
            FocusFrame { shown: chip.visualFocus }
        }
        contentItem: RowLayout {
            spacing: 8
            DeckIcon {
                name: chip.symbol
                ink: !chip.enabled ? panel.muted : chip.primary ? panel.surface : chip.tone
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                Layout.alignment: Qt.AlignCenter
            }
            Text {
                visible: text.length > 0
                text: chip.text
                color: !chip.enabled ? panel.muted : chip.primary ? panel.surface : chip.tone
                font.family: panel.textFont.family
                font.pointSize: panel.textFont.pointSize
                font.weight: Font.DemiBold
            }
        }
    }
    component InstallationSelector: RowLayout {
        id: selector
        enabled: panel.actionEnabled
        spacing: 4
        Layout.alignment: Qt.AlignVCenter
        property int currentIndex: panel.installationIndex
        readonly property string currentText: currentIndex >= 0 && currentIndex < panel.installationChoices.length
            ? panel.installationChoices[currentIndex].label : ""
        readonly property int count: panel.installationChoices.length
        Repeater {
            model: panel.installationChoices
            delegate: Controls.Button {
                id: choice
                required property var modelData
                required property int index
                objectName: "installationChoice" + modelData.label
                text: modelData.label
                // Not checkable: clicking only reports the choice, so `checked`
                // stays bound to the selection instead of a click breaking it.
                checkable: false
                checked: index === selector.currentIndex
                enabled: selector.enabled
                Accessible.name: modelData.label + " Flatpak"
                implicitHeight: Math.max(32, Theme.controlHeight - 4)
                leftPadding: 10
                rightPadding: 10
                font.pointSize: Theme.pointSize(Theme.smallScale)
                font.weight: checked ? Font.DemiBold : Font.Normal
                onClicked: panel.installationRequested(index)
                background: Rectangle {
                    radius: Theme.controlRadius
                    color: !choice.enabled ? "transparent"
                        : choice.checked ? Theme.tint(panel.accent, choice.down ? 0.24 : 0.16)
                        : (choice.hovered ? Theme.hoverTint : "transparent")
                    border.color: choice.visualFocus ? panel.accent
                        : choice.checked ? Theme.tint(panel.accent, 0.45) : panel.line
                    border.width: choice.visualFocus ? 2 : 1
                }
                contentItem: Text {
                    text: choice.text
                    color: choice.enabled ? panel.ink : panel.muted
                    font: choice.font
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
            }
        }
    }
    readonly property string homepage: detailMatchesSelection && detailsData.homepage ? String(detailsData.homepage) : ""
    readonly property bool homepageOpens: /^https?:\/\//i.test(homepage)
    readonly property var dependencies: detailMatchesSelection ? (detailsData.dependencies || []) : []
    readonly property var facts: detailMatchesSelection ? [
        {label: "Updates", value: updateState},
        {label: "Publisher", value: detailsData.publisher || ""},
        {label: "License", value: detailsData.license || ""},
        {label: "File", value: location},
        {label: "Size", value: appFile ? sizeText(appFile.bytes) : ""},
        {label: "Updated", value: appFile && appFile.modified > 0 ? new Date(appFile.modified * 1000).toLocaleDateString(Qt.locale(), Locale.ShortFormat) : ""}
    ].filter(fact => fact.value) : []
    // The source's display name, under the package name.
    readonly property string subtitle: selected && selected.kind === "package" && selected.source
        ? String(sourceName(selected.source) || selected.source) : ""

    color: surface
    radius: Theme.cardRadius
    border.color: line

    // Keyboard focus rings only; a mouse click leaves none.
    component FocusFrame: Rectangle {
        property bool shown: false
        anchors.fill: parent
        anchors.margins: -2
        radius: Theme.controlRadius + 2
        color: "transparent"
        border.color: panel.accent
        border.width: 2
        visible: shown
    }

    ColumnLayout {
        id: detailsContent
        objectName: "detailsContent"
        anchors.fill: parent
        anchors.margins: panel.page && !panel.embedded ? Theme.gutter * 1.5 : Theme.gutter
        spacing: panel.page && !panel.embedded ? Theme.spacingLarge : Theme.spacingSmall
        RowLayout {
            id: header
            objectName: "detailsHeader"
            Layout.fillWidth: true
            spacing: Theme.spacing
            Controls.Button {
                id: backButton
                objectName: "pageBackButton"
                visible: panel.page && !panel.embedded
                Accessible.name: "Back"
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 500
                Controls.ToolTip.text: Accessible.name
                Layout.preferredWidth: 38
                Layout.alignment: Qt.AlignTop
                implicitHeight: 38
                onClicked: panel.closeRequested()
                background: Rectangle {
                    color: backButton.down ? Theme.tint(panel.ink, 0.12) : backButton.hovered ? Theme.tint(panel.ink, 0.08) : "transparent"
                    radius: Theme.controlRadius
                    border.color: backButton.visualFocus ? panel.accent : "transparent"
                    border.width: 2
                    Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
                }
                contentItem: Item {
                    DeckIcon { anchors.centerIn: parent; name: "back"; ink: panel.ink; width: 18; height: 18 }
                }
            }
            Item {
                objectName: "detailsIcon"
                Layout.preferredWidth: panel.iconSize
                Layout.preferredHeight: panel.iconSize
                Layout.alignment: Qt.AlignVCenter
                Rectangle {
                    anchors.fill: parent
                    radius: Theme.controlRadius
                    color: Theme.tint(panel.accent, 0.1)
                    visible: detailIcon.status !== Image.Ready
                    DeckIcon {
                        name: panel.selected ? (panel.selected.kind === "source" ? panel.selected.source : (panel.selected.kind === "failure" ? "warning" : "package")) : "package"
                        ink: panel.accent
                        anchors.centerIn: parent
                        width: panel.iconSize * 0.55
                        height: panel.iconSize * 0.55
                    }
                }
                Image {
                    id: detailIcon
                    anchors.fill: parent
                    visible: status === Image.Ready
                    asynchronous: true
                    source: panel.iconSource
                    sourceSize.width: Math.round(panel.iconSize * Screen.devicePixelRatio)
                    sourceSize.height: Math.round(panel.iconSize * Screen.devicePixelRatio)
                    fillMode: Image.PreserveAspectFit
                    Accessible.ignored: true
                }
                // Where the package comes from, on the icon's corner.
                Rectangle {
                    objectName: "detailsSourceBadge"
                    visible: !!panel.selected && panel.selected.kind === "package" && !!panel.selected.source
                    width: 20
                    height: 20
                    radius: 10
                    x: parent.width - width + 4
                    y: parent.height - height + 4
                    color: panel.surface
                    border.color: panel.line
                    DeckIcon {
                        anchors.centerIn: parent
                        width: 13
                        height: 13
                        name: panel.selected && panel.selected.source ? panel.selected.source : "package"
                        ink: panel.muted
                    }
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                spacing: 2
                Controls.Label {
                    objectName: "detailsTitle"
                    text: panel.selected ? (panel.selected.display_name || panel.selected.name || "") : ""
                    textFormat: Text.PlainText
                    color: panel.ink
                    font.pointSize: Theme.pointSize(panel.compact ? Theme.titleScale : (panel.page ? Theme.titleScale * 1.35 : Theme.titleScale))
                    font.weight: Font.DemiBold
                    wrapMode: panel.compact ? Text.WordWrap : Text.NoWrap
                    maximumLineCount: panel.compact ? 2 : 1
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                RowLayout {
                    spacing: Theme.spacingSmall
                    Layout.fillWidth: true
                    visible: panel.subtitle.length > 0 || (panel.page && panel.version.length > 0)
                        || (panel.compact && panel.installationChoices.length > 1)
                    Controls.Label {
                        objectName: "detailsSubtitle"
                        visible: text.length > 0
                        text: panel.subtitle
                        textFormat: Text.PlainText
                        color: panel.muted
                        font.pointSize: Theme.pointSize(Theme.smallScale)
                        elide: Text.ElideRight
                        // Rounded up: layouts give whole pixels, and a width
                        // just under the text's fractional one elides it.
                        Layout.maximumWidth: Math.ceil(implicitWidth)
                        Layout.fillWidth: true
                    }
                    Controls.Label {
                        objectName: "pageVersion"
                        visible: panel.page && panel.version.length > 0
                        text: panel.version
                        textFormat: Text.PlainText
                        color: panel.muted
                        font.family: "monospace"
                        font.pointSize: Theme.pointSize(Theme.smallScale)
                    }
                    InstallationSelector {
                        objectName: "compactInstallationSelector"
                        visible: panel.compact && panel.installationChoices.length > 1
                    }
                    Item { Layout.fillWidth: true; visible: !panel.compact }
                }
            }
            InstallationSelector {
                objectName: "packageInstallationSelector"
                visible: !panel.compact && panel.installationChoices.length > 1
            }
            DetailsAction {
                objectName: "pageLaunchButton"
                label: panel.launchText
                symbol: "launch"
                tone: panel.accent
                primary: panel.launchIsMain
                enabled: panel.actionEnabled
                onClicked: panel.launchRequested()
            }
            DetailsAction {
                id: secondaryActionButton
                objectName: "detailsSecondaryActionButton"
                label: panel.secondaryActionText
                symbol: panel.secondaryActionSymbol
                tone: panel.secondaryActionColor
                enabled: panel.actionEnabled
                onClicked: panel.secondaryActionRequested()
            }
            DetailsAction {
                id: actionButton
                objectName: "detailsActionButton"
                label: panel.actionText
                iconOnly: panel.actionSymbol === "remove"
                symbol: panel.actionSymbol
                tone: panel.actionColor
                primary: panel.page && !panel.launchIsMain && panel.actionTone !== "danger"
                accessibleName: panel.actionAccessibleName
                enabled: panel.actionEnabled
                onClicked: panel.actionRequested()
            }
            Controls.Button {
                id: closeButton
                objectName: "closeDetailsButton"
                visible: !panel.page || panel.embedded
                Accessible.name: "Close details"
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 500
                Controls.ToolTip.text: Accessible.name
                Layout.preferredWidth: 38
                Layout.alignment: Qt.AlignVCenter
                implicitHeight: 38
                onClicked: panel.closeRequested()
                background: Rectangle {
                    color: closeButton.down ? Theme.tint(panel.ink, 0.12) : closeButton.hovered ? Theme.tint(panel.ink, 0.08) : "transparent"
                    radius: Theme.controlRadius
                    border.color: closeButton.visualFocus ? panel.accent : "transparent"
                    border.width: 2
                    Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
                }
                // A content item fills the button; keep the icon at its size.
                contentItem: Item {
                    DeckIcon { objectName: "closeDetailsIcon"; anchors.centerIn: parent; name: "cancel"; ink: panel.muted; width: 16; height: 16 }
                }
            }
        }
        // What the page's own action does, and how the app starts here.
        Controls.Label {
            objectName: "pageNote"
            visible: panel.page && text.length > 0
            Layout.fillWidth: true
            text: panel.note
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: panel.muted
        }
        // Why Launch failed, when its settings aren't on this page to show it.
        Controls.Label {
            objectName: "pageLaunchError"
            visible: panel.page && text.length > 0 && !(panel.launchSettings && panel.launchSettings.editable === true)
            Layout.fillWidth: true
            text: panel.launchError
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.danger
        }
        Controls.Label {
            objectName: "pageFuseNote"
            visible: panel.page && !!panel.appFile && panel.appFile.without_fuse === true
            Layout.fillWidth: true
            text: "Starts unpacked: this computer doesn't have FUSE 2 (libfuse2), which this AppImage needs to start the usual way."
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.warning
        }
        DeckScrollView {
            ink: panel.muted
            id: detailScroll
            objectName: "detailsScroll"
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            ColumnLayout {
                id: detailBody
                width: detailScroll.availableWidth
                spacing: Theme.spacing
                // Changes to look at before they run, on the page itself.
                Rectangle {
                    id: reviewCard
                    objectName: "pageReview"
                    visible: panel.page && panel.reviewSummary.length > 0
                    Layout.fillWidth: true
                    implicitHeight: reviewColumn.implicitHeight + 2 * Theme.gutter
                    radius: Theme.cardRadius
                    color: Theme.tint(panel.reviewDanger ? Theme.danger : panel.accent, 0.1)
                    border.color: Theme.tint(panel.reviewDanger ? Theme.danger : panel.accent, 0.35)
                    ColumnLayout {
                        id: reviewColumn
                        anchors.fill: parent
                        anchors.margins: Theme.gutter
                        spacing: Theme.spacingSmall
                        Controls.Label {
                            objectName: "pageReviewTitle"
                            Layout.fillWidth: true
                            text: panel.reviewSummary.split("\n")[0]
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                            color: panel.ink
                            font.weight: Font.DemiBold
                        }
                        Controls.Label {
                            objectName: "pageReviewMeta"
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: panel.reviewSummary.split("\n").slice(1).join("\n").trim()
                            textFormat: Text.PlainText
                            wrapMode: Text.Wrap
                            color: panel.muted
                        }
                        TextEdit {
                            objectName: "pageReviewDetails"
                            Layout.fillWidth: true
                            visible: text.length > 0
                            text: panel.reviewDetails
                            readOnly: true
                            selectByMouse: true
                            wrapMode: TextEdit.Wrap
                            textFormat: TextEdit.PlainText
                            color: panel.muted
                            font.pointSize: Theme.pointSize(Theme.smallScale)
                        }
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacingSmall
                            Item { Layout.fillWidth: true }
                            DetailsAction {
                                objectName: "pageReviewApply"
                                label: panel.reviewActionText || "Apply"
                                symbol: panel.reviewDanger ? "remove" : "install"
                                tone: panel.reviewDanger ? Theme.danger : panel.accent
                                onClicked: panel.reviewAccepted()
                            }
                            DetailsAction {
                                objectName: "pageReviewCancel"
                                label: "Cancel"
                                symbol: "cancel"
                                tone: panel.muted
                                onClicked: panel.reviewRejected()
                            }
                        }
                    }
                }
                ColumnLayout {
                    id: loadedBody
                    objectName: "detailsBody"
                    visible: !panel.loading
                    Layout.fillWidth: true
                    spacing: Theme.spacing
                    // Details fade in when they arrive.
                    opacity: panel.loading ? 0 : 1
                    Behavior on opacity {
                        enabled: panel.motionEnabled
                        NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic }
                    }
                    ListView {
                        boundsBehavior: Theme.motionEnabled ? Flickable.DragAndOvershootBounds : Flickable.StopAtBounds
                        id: screenshotGallery
                        objectName: "screenshotGallery"
                        visible: panel.screenshots.length > 0
                        Layout.fillWidth: true
                        Layout.preferredHeight: panel.compact ? 72 : panel.thumbnailHeight
                        orientation: ListView.Horizontal
                        spacing: Theme.spacing
                        clip: true
                        model: panel.screenshots
                        cacheBuffer: 0
                        reuseItems: true
                        delegate: Controls.AbstractButton {
                            id: thumbnail
                            required property var modelData
                            width: panel.compact ? 128 : panel.thumbnailWidth
                            height: screenshotGallery.height
                            enabled: preview.status === Image.Ready
                            Accessible.name: modelData.caption || "View app screenshot"
                            Controls.ToolTip.visible: hovered
                            Controls.ToolTip.delay: 500
                            Controls.ToolTip.text: Accessible.name
                            onClicked: panel.screenshotRequested(modelData.url, modelData.caption || "")
                            background: Rectangle {
                                color: panel.canvas
                                radius: Theme.controlRadius
                                border.color: thumbnail.visualFocus ? panel.accent : thumbnail.hovered ? Theme.strongLine : panel.line
                                border.width: thumbnail.visualFocus ? 2 : 1
                            }
                            contentItem: Item {
                                Image {
                                    id: preview
                                    onStatusChanged: {
                                        if (status === Image.Error)
                                            Qt.callLater(panel.screenshotFailed, thumbnail.modelData.url, panel.selectionIdentity);
                                    }
                                    anchors.fill: parent
                                    anchors.margins: 1
                                    source: panel.detailMatchesSelection ? thumbnail.modelData.url : ""
                                    asynchronous: true
                                    cache: true
                                    // Decode at the largest thumbnail size shown, not the
                                    // full image. A fixed size keeps a compact/wide switch
                                    // from reloading every image.
                                    sourceSize.width: Math.round(panel.thumbnailWidth * Screen.devicePixelRatio)
                                    sourceSize.height: Math.round(panel.thumbnailHeight * Screen.devicePixelRatio)
                                    fillMode: Image.PreserveAspectFit
                                    opacity: status === Image.Ready ? 1 : 0
                                    Behavior on opacity {
                                        enabled: panel.motionEnabled
                                        NumberAnimation { duration: Theme.revealDuration }
                                    }
                                }
                                // A soft placeholder while the image loads.
                                Rectangle {
                                    id: previewPlaceholder
                                    anchors.fill: parent
                                    anchors.margins: 6
                                    radius: Theme.smallRadius
                                    color: Theme.tint(panel.ink, 0.06)
                                    visible: preview.status === Image.Loading
                                    SequentialAnimation on opacity {
                                        running: previewPlaceholder.visible && panel.motionEnabled && Theme.pulseDuration > 0
                                        loops: Animation.Infinite
                                        NumberAnimation { from: 1; to: 0.5; duration: Math.max(1, Theme.pulseDuration) }
                                        NumberAnimation { from: 0.5; to: 1; duration: Math.max(1, Theme.pulseDuration) }
                                    }
                                }
                            }
                        }
                    }
                    Controls.Label {
                        objectName: "pageSummary"
                        visible: panel.summary.length > 0
                        Layout.fillWidth: true
                        text: panel.summary
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: panel.ink
                        font.pointSize: Theme.pointSize(Theme.titleScale)
                        font.weight: Font.DemiBold
                    }
                    TextEdit {
                        objectName: "packageDetails"
                        visible: panel.description.length > 0
                        Layout.fillWidth: true
                        readOnly: true
                        selectByMouse: true
                        color: panel.ink
                        opacity: 0.86
                        padding: 0
                        font.pointSize: panel.textFont.pointSize
                        wrapMode: TextEdit.Wrap
                        textFormat: TextEdit.PlainText
                        text: panel.description
                        Accessible.name: "Package details"
                    }
                    // Facts as a two-column grid, then the dependency list.
                    ColumnLayout {
                        objectName: "packageMetadata"
                        // The same facts as plain text, for tests and copying.
                        readonly property string text: panel.metadataText
                        visible: panel.metadataText.length > 0 || panel.facts.length > 0
                        Layout.fillWidth: true
                        Layout.topMargin: panel.description.length > 0 || panel.screenshots.length > 0 ? 2 : 0
                        spacing: Theme.spacing
                        Accessible.name: "Additional package metadata"
                        GridLayout {
                            objectName: "detailsFacts"
                            visible: panel.facts.length > 0 || panel.homepage.length > 0
                            columns: 2
                            columnSpacing: Theme.spacingLarge
                            rowSpacing: Theme.spacingSmall
                            Layout.fillWidth: true
                            Repeater {
                                model: panel.facts
                                delegate: Controls.Label {
                                    required property var modelData
                                    required property int index
                                    Layout.row: index
                                    Layout.column: 0
                                    Layout.alignment: Qt.AlignTop
                                    text: modelData.label
                                    color: panel.muted
                                    font.pointSize: Theme.pointSize(Theme.smallScale)
                                }
                            }
                            Repeater {
                                model: panel.facts
                                delegate: TextEdit {
                                    required property var modelData
                                    required property int index
                                    Layout.row: index
                                    Layout.column: 1
                                    Layout.fillWidth: true
                                    text: modelData.value
                                    textFormat: TextEdit.PlainText
                                    readOnly: true
                                    selectByMouse: true
                                    wrapMode: TextEdit.Wrap
                                    color: panel.ink
                                    font.pointSize: Theme.pointSize(Theme.smallScale)
                                    Accessible.name: modelData.label + ": " + modelData.value
                                }
                            }
                            Controls.Label {
                                visible: panel.homepage.length > 0
                                Layout.row: panel.facts.length
                                Layout.column: 0
                                Layout.alignment: Qt.AlignVCenter
                                text: "Homepage"
                                color: panel.muted
                                font.pointSize: Theme.pointSize(Theme.smallScale)
                            }
                            Controls.AbstractButton {
                                id: homepageLink
                                objectName: "homepageLink"
                                visible: panel.homepage.length > 0
                                enabled: panel.homepageOpens
                                Layout.row: panel.facts.length
                                Layout.column: 1
                                Layout.fillWidth: true
                                Layout.maximumWidth: Math.ceil(implicitWidth)
                                hoverEnabled: true
                                Accessible.role: Accessible.Link
                                Accessible.name: "Homepage " + panel.homepage
                                Controls.ToolTip.visible: hovered
                                Controls.ToolTip.delay: 500
                                Controls.ToolTip.text: panel.homepage
                                onClicked: Qt.openUrlExternally(panel.homepage)
                                HoverHandler { cursorShape: homepageLink.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
                                background: FocusFrame { shown: homepageLink.visualFocus; radius: Theme.smallRadius }
                                contentItem: RowLayout {
                                    spacing: 5
                                    Text {
                                        text: panel.homepage.replace(/^https?:\/\//i, "").replace(/\/$/, "")
                                        textFormat: Text.PlainText
                                        elide: Text.ElideMiddle
                                        color: homepageLink.enabled ? panel.accent : panel.ink
                                        font.pointSize: Theme.pointSize(Theme.smallScale)
                                        font.underline: homepageLink.hovered && homepageLink.enabled
                                        Layout.fillWidth: true
                                    }
                                    DeckIcon {
                                        visible: homepageLink.enabled
                                        name: "external"
                                        ink: panel.accent
                                        Layout.preferredWidth: 13
                                        Layout.preferredHeight: 13
                                    }
                                }
                            }
                        }
                        DetailsAction {
                            objectName: "showInFolder"
                            label: panel.appFile && panel.appFile.folder ? "Show in folder" : ""
                            symbol: "external"
                            tone: panel.accent
                            accessibleName: "Show " + (panel.selected ? (panel.selected.display_name || panel.selected.name || "") : "") + " in its folder"
                            onClicked: panel.showInFolderRequested(panel.appFile.folder)
                        }
                        // Collapsed by default: long APT lists stay out of the way.
                        Controls.AbstractButton {
                            id: dependenciesToggle
                            objectName: "dependenciesToggle"
                            visible: panel.dependencies.length > 0
                            Layout.fillWidth: true
                            hoverEnabled: true
                            Accessible.role: Accessible.Button
                            Accessible.name: (panel.dependenciesExpanded ? "Hide " : "Show ") + "dependencies (" + panel.dependencies.length + ")"
                            onClicked: panel.dependenciesExpanded = !panel.dependenciesExpanded
                            background: FocusFrame { shown: dependenciesToggle.visualFocus; radius: Theme.smallRadius }
                            contentItem: RowLayout {
                                spacing: 6
                                DeckIcon {
                                    name: "right"
                                    ink: panel.muted
                                    rotation: panel.dependenciesExpanded ? 90 : 0
                                    Layout.preferredWidth: 13
                                    Layout.preferredHeight: 13
                                    Behavior on rotation {
                                        enabled: panel.motionEnabled
                                        NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic }
                                    }
                                }
                                Text {
                                    objectName: "dependenciesHeading"
                                    text: "Dependencies (" + panel.dependencies.length + ")"
                                    color: dependenciesToggle.hovered ? panel.ink : panel.muted
                                    font.pointSize: Theme.pointSize(Theme.smallScale)
                                    font.weight: Font.DemiBold
                                    Layout.fillWidth: true
                                }
                            }
                        }
                        Flow {
                            id: dependencyChips
                            objectName: "dependencyChips"
                            visible: panel.dependenciesExpanded && panel.dependencies.length > 0
                            Layout.fillWidth: true
                            spacing: 6
                            opacity: visible ? 1 : 0
                            Behavior on opacity {
                                enabled: panel.motionEnabled
                                NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic }
                            }
                            Repeater {
                                model: dependencyChips.visible ? panel.dependencies : []
                                delegate: Controls.Label {
                                    required property var modelData
                                    text: String(modelData)
                                    textFormat: Text.PlainText
                                    color: panel.ink
                                    font.pointSize: Theme.pointSize(Theme.captionScale)
                                    elide: Text.ElideRight
                                    width: Math.min(implicitWidth, dependencyChips.width)
                                    leftPadding: 8
                                    rightPadding: 8
                                    topPadding: 3
                                    bottomPadding: 3
                                    background: Rectangle {
                                        radius: Theme.smallRadius
                                        color: Theme.tint(panel.ink, 0.06)
                                        border.color: panel.line
                                    }
                                }
                            }
                        }
                    }
                    // How an AppImage PkgDeck manages starts, from here and
                    // from the app menu: arguments and environment variables.
                    ColumnLayout {
                        id: launchEditor
                        objectName: "launchSettings"
                        visible: panel.page && !!panel.launchSettings && panel.launchSettings.editable === true
                        Layout.fillWidth: true
                        spacing: Theme.spacingSmall
                        property var environment: []
                        property bool dirty: false
                        function reset() {
                            const settings = panel.launchSettings || {};
                            argumentsField.text = settings.arguments || "";
                            environment = (settings.environment || []).map(pair => ({name: pair.name, value: pair.value}));
                            dirty = false;
                        }
                        Connections {
                            target: panel
                            function onLaunchSettingsChanged() { launchEditor.reset(); }
                        }
                        Component.onCompleted: reset()
                        Controls.Label {
                            text: "Command line arguments"
                            color: panel.muted
                            font.pointSize: Theme.pointSize(Theme.smallScale)
                        }
                        ThemedTextField {
                            id: argumentsField
                            objectName: "launchArguments"
                            Layout.fillWidth: true
                            font.family: "monospace"
                            Accessible.name: "Command line arguments"
                            onTextEdited: launchEditor.dirty = true
                        }
                        Controls.Label {
                            text: "Environment variables"
                            color: panel.muted
                            font.pointSize: Theme.pointSize(Theme.smallScale)
                            Layout.topMargin: Theme.spacingSmall
                        }
                        Repeater {
                            objectName: "launchEnvironment"
                            model: launchEditor.environment
                            delegate: RowLayout {
                                required property var modelData
                                required property int index
                                Layout.fillWidth: true
                                spacing: Theme.spacingSmall
                                ThemedTextField {
                                    objectName: "environmentName"
                                    Layout.preferredWidth: 220
                                    text: modelData.name
                                    font.family: "monospace"
                                    placeholderText: "NAME"
                                    Accessible.name: "Variable name"
                                    onTextEdited: { launchEditor.environment[index].name = text; launchEditor.dirty = true; }
                                }
                                Controls.Label { text: "="; color: panel.muted }
                                ThemedTextField {
                                    objectName: "environmentValue"
                                    Layout.fillWidth: true
                                    text: modelData.value
                                    font.family: "monospace"
                                    Accessible.name: "Variable value"
                                    onTextEdited: { launchEditor.environment[index].value = text; launchEditor.dirty = true; }
                                }
                                DetailsAction {
                                    objectName: "removeVariable"
                                    label: "Remove variable"
                                    iconOnly: true
                                    symbol: "remove"
                                    tone: Theme.danger
                                    accessibleName: "Remove " + (modelData.name || "variable")
                                    // Flag the change first: the new list rebuilds this row.
                                    onClicked: {
                                        launchEditor.dirty = true;
                                        launchEditor.environment = launchEditor.environment.filter((_, i) => i !== index);
                                    }
                                }
                            }
                        }
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacingSmall
                            DetailsAction {
                                objectName: "addVariable"
                                label: "Add variable"
                                symbol: "add"
                                tone: panel.accent
                                onClicked: {
                                    launchEditor.dirty = true;
                                    launchEditor.environment = launchEditor.environment.concat([{name: "", value: ""}]);
                                }
                            }
                            Item { Layout.fillWidth: true }
                            DetailsAction {
                                objectName: "saveLaunchSettings"
                                label: "Save"
                                symbol: "installed"
                                tone: panel.accent
                                enabled: launchEditor.dirty
                                onClicked: panel.launchSettingsSaved(argumentsField.text, launchEditor.environment)
                            }
                        }
                        Controls.Label {
                            objectName: "launchError"
                            visible: text.length > 0
                            Layout.fillWidth: true
                            text: panel.launchError
                            wrapMode: Text.Wrap
                            color: Theme.danger
                        }
                    }
                    // A GitHub project's releases, for AppImages that can't
                    // update themselves (or update from somewhere better).
                    ColumnLayout {
                        id: updateEditor
                        objectName: "updateSource"
                        visible: panel.page && !!panel.updateSource && panel.updateSource.editable === true
                        Layout.fillWidth: true
                        spacing: Theme.spacingSmall
                        property bool dirty: false
                        function reset() {
                            githubField.text = (panel.updateSource && panel.updateSource.github) || "";
                            dirty = false;
                        }
                        Connections {
                            target: panel
                            function onUpdateSourceChanged() { updateEditor.reset(); }
                        }
                        Component.onCompleted: reset()
                        Controls.Label {
                            text: "Updates from GitHub"
                            color: panel.muted
                            font.pointSize: Theme.pointSize(Theme.smallScale)
                        }
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Theme.spacingSmall
                            ThemedTextField {
                                id: githubField
                                objectName: "githubRepository"
                                Layout.fillWidth: true
                                placeholderText: "owner/name"
                                Accessible.name: "GitHub project"
                                onTextEdited: updateEditor.dirty = true
                                onAccepted: if (updateEditor.dirty) panel.updateSourceSaved(text)
                            }
                            DetailsAction {
                                objectName: "saveUpdateSource"
                                label: "Save"
                                symbol: "installed"
                                tone: panel.accent
                                enabled: updateEditor.dirty
                                onClicked: panel.updateSourceSaved(githubField.text)
                            }
                        }
                        Controls.Label {
                            objectName: "updateError"
                            visible: text.length > 0
                            Layout.fillWidth: true
                            text: panel.updateError
                            wrapMode: Text.Wrap
                            color: Theme.danger
                        }
                    }
                }
                // Placeholder bars while the selected row's details load, below
                // whatever part of them is already in.
                Column {
                    id: skeleton
                    objectName: "detailsSkeleton"
                    visible: panel.loading || panel.loadingMore
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    spacing: 10
                    readonly property bool pulsing: pulse.running
                    Repeater {
                        model: [0.92, 1, 0.78, 0.46]
                        Rectangle {
                            required property real modelData
                            width: skeleton.width * modelData
                            height: 10
                            radius: 5
                            color: Theme.tint(panel.ink, 0.09)
                        }
                    }
                    SequentialAnimation on opacity {
                        id: pulse
                        running: skeleton.visible && panel.motionEnabled && Theme.pulseDuration > 0
                        loops: Animation.Infinite
                        NumberAnimation { from: 1; to: 0.45; duration: Math.max(1, Theme.pulseDuration); easing.type: Easing.InOutSine }
                        NumberAnimation { from: 0.45; to: 1; duration: Math.max(1, Theme.pulseDuration); easing.type: Easing.InOutSine }
                        onRunningChanged: if (!running) skeleton.opacity = 1
                    }
                }
            }
        }
    }
}
