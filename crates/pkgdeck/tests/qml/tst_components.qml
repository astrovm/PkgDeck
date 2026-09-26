import QtQuick
import QtQuick.Controls as Controls
import QtTest
import "../../qml" as App

TestCase {
    id: test
    name: "Components"
    when: windowShown
    // TestCase hides itself by default; components under test must render.
    visible: true
    width: 800
    height: 600

    Component {
        id: toastComponent
        App.Toast { timeout: 150 }
    }
    Component {
        id: rowProgressComponent
        App.RowProgress { width: 200 }
    }
    Component {
        id: actionProgressComponent
        App.ActionProgress { width: 600 }
    }
    Component {
        id: detailsComponent
        App.PackageDetails { width: 600; height: 400 }
    }
    Component {
        id: activityComponent
        App.ActivityPane { width: 600; height: 400 }
    }
    Component {
        id: scrollComponent
        ListView {
            width: 200
            height: 200
            model: 100
            delegate: Rectangle { width: 180; height: 30 }
            Controls.ScrollBar.vertical: App.DeckScrollBar { lingerInterval: 80 }
        }
    }
    Component {
        id: scrollViewComponent
        App.DeckScrollView {
            width: 200
            height: 200
            Column {
                Repeater { model: 30; Rectangle { width: 150; height: 30 } }
            }
        }
    }
    SignalSpy { id: spyA }
    SignalSpy { id: spyB }

    function init() {
        App.Theme.reduceMotion = false;
        for (const spy of [spyA, spyB]) {
            spy.signalName = "";
            spy.target = null;
            spy.clear();
        }
    }
    function cleanup() {
        App.Theme.reduceMotion = false;
    }
    function row(name, extra) {
        return Object.assign({kind: "package", name: name, source: "apt", installed: "", candidate: "1.0", scope: "system"}, extra || {});
    }

    function test_motion_off_zeroes_every_duration() {
        verify(App.Theme.motionEnabled);
        verify(App.Theme.feedbackDuration > 0);
        verify(App.Theme.pulseDuration > 0);
        App.Theme.reduceMotion = true;
        verify(!App.Theme.motionEnabled);
        for (const key of ["feedbackDuration", "revealDuration", "layoutDuration", "pulseDuration"])
            compare(App.Theme[key], 0, key);
    }

    function test_toast_shows_hides_and_runs_its_action() {
        const toast = createTemporaryObject(toastComponent, test);
        verify(!toast.open);
        verify(!toast.visible);
        compare(toast.Accessible.role, Accessible.AlertMessage);
        spyA.target = toast;
        spyA.signalName = "actionTriggered";
        spyB.target = toast;
        spyB.signalName = "dismissed";
        toast.timeout = 60000;
        toast.show("Removed synthetic-tool", "Undo", "success");
        verify(toast.open);
        verify(toast.visible);
        compare(toast.text, "Removed synthetic-tool");
        compare(findChild(toast, "toastText").text, "Removed synthetic-tool");
        const action = findChild(toast, "toastAction");
        verify(action.visible);
        compare(action.text, "Undo");
        compare(toast.toneColor, App.Theme.success);
        waitForRendering(toast);
        mouseClick(action);
        compare(spyA.count, 1);
        compare(spyB.count, 1);
        verify(!toast.open);
        // The exit fades out rather than vanishing.
        tryCompare(toast, "visible", false);

        // Without an action there is no action button; close dismisses.
        toast.show("Update finished");
        compare(toast.tone, "info");
        verify(!findChild(toast, "toastAction").visible);
        waitForRendering(toast);
        mouseClick(findChild(toast, "toastClose"));
        compare(spyB.count, 2);
        compare(spyA.count, 1);
        verify(!toast.open);
    }

    function test_toast_hides_itself_but_waits_while_hovered() {
        const toast = createTemporaryObject(toastComponent, test);
        spyB.target = toast;
        spyB.signalName = "dismissed";
        toast.show("Installed synthetic-tool");
        tryCompare(toast, "open", false, 2000);
        compare(spyB.count, 1);
        tryCompare(toast, "visible", false);

        toast.show("Installed synthetic-tool");
        waitForRendering(toast);
        mouseMove(findChild(toast, "toastCard"), 20, 10);
        tryCompare(toast, "hovered", true);
        wait(400);
        verify(toast.open, "a hovered toast stays");
        mouseMove(test, test.width - 2, test.height - 2);
        tryCompare(toast, "open", false, 2000);
    }

    function test_toast_without_motion_shows_and_hides_at_once() {
        App.Theme.reduceMotion = true;
        const toast = createTemporaryObject(toastComponent, test);
        toast.timeout = 60000;
        toast.show("Saved");
        compare(findChild(toast, "toastCard").opacity, 1);
        verify(toast.visible);
        toast.hide();
        compare(findChild(toast, "toastCard").opacity, 0);
        verify(!toast.visible);
    }

    function test_row_progress_fills_or_sweeps() {
        App.Theme.reduceMotion = true;
        const bar = createTemporaryObject(rowProgressComponent, test, {value: 0.5});
        compare(bar.height, 3);
        verify(!bar.indeterminate);
        verify(!findChild(bar, "rowProgressSegment").visible);
        bar.value = -1;
        verify(bar.indeterminate);
        verify(findChild(bar, "rowProgressSegment").visible);
        // Motion off: a still segment in the middle, no sweep.
        verify(!bar.sweeping);
        const segment = findChild(bar, "rowProgressSegment");
        compare(segment.x, (bar.width - segment.width) / 2);
        App.Theme.reduceMotion = false;
        tryCompare(bar, "sweeping", true);
        bar.running = false;
        verify(!bar.sweeping);
        tryCompare(bar, "visible", false);
        bar.running = true;
        App.Theme.reduceMotion = true;
        verify(!bar.sweeping);
        compare(bar.clampedValue, 0);
        bar.value = 3;
        compare(bar.clampedValue, 1);
    }

    function test_action_progress_is_one_line_with_optional_cancel() {
        const progress = createTemporaryObject(actionProgressComponent, test,
            {label: "Update synthetic-tool from apt", done: 2, total: 3});
        waitForRendering(progress);
        const bar = findChild(progress, "actionProgressBar");
        const label = findChild(progress, "actionProgressLabel");
        verify(!progress.stacked);
        compare(bar.value, 2);
        compare(bar.to, 3);
        compare(findChild(progress, "actionProgressCount").text, "2 of 3");
        // Label and bar share the line; the bar fills what the label leaves.
        const labelBox = label.mapToItem(progress, 0, 0);
        const barBox = bar.mapToItem(progress, 0, 0);
        verify(barBox.x >= labelBox.x + label.width);
        verify(bar.width > progress.width * 0.3);
        verify(Math.abs((barBox.y + bar.height / 2) - (labelBox.y + label.height / 2)) <= 2);
        verify(!findChild(progress, "actionProgressCancel").visible);
        progress.cancelable = true;
        spyA.target = progress;
        spyA.signalName = "cancelRequested";
        waitForRendering(progress);
        mouseClick(findChild(progress, "actionProgressCancel"));
        compare(spyA.count, 1);
        // Transfers report a percentage.
        progress.transferred = 25;
        progress.transferTotal = 100;
        compare(findChild(progress, "actionProgressCount").text, "25%");
        // Narrow: the label sits above a full-width bar.
        progress.width = 300;
        waitForRendering(progress);
        verify(progress.stacked);
        verify(bar.mapToItem(progress, 0, 0).y > label.mapToItem(progress, 0, 0).y + label.height - 1);
    }

    function test_action_progress_indeterminate_follows_motion() {
        const progress = createTemporaryObject(actionProgressComponent, test, {label: "Install", done: 0, total: 1});
        verify(progress.indeterminate);
        verify(findChild(progress, "actionProgressSweep").visible);
        verify(!findChild(progress, "actionProgressStripes").visible);
        App.Theme.reduceMotion = true;
        verify(!findChild(progress, "actionProgressSweep").visible);
        verify(findChild(progress, "actionProgressStripes").visible);
    }

    function test_details_keep_header_and_show_skeleton_while_loading() {
        const details = createTemporaryObject(detailsComponent, test);
        const first = row("synthetic-tool", {display_name: "Synthetic Tool"});
        details.selected = first;
        details.detailMatchesSelection = true;
        details.detailsData = {package: first, publisher: "Example publisher", license: "MIT",
            homepage: "https://example.invalid/app", dependencies: ["libc6 (>= 2.34)", "synthetic-library", "zlib1g"]};
        details.description = Array(12).fill("A long synthetic description line.").join("\n");
        waitForRendering(details);
        verify(!details.loading);
        verify(!findChild(details, "detailsSkeleton").visible);
        const settled = details.idealHeight;
        verify(settled > 200);
        verify(details.hasDetails);

        // Another row: the header follows at once, the body loads.
        details.selected = row("other-tool");
        details.detailMatchesSelection = false;
        details.detailsData = ({});
        details.description = "";
        verify(details.loading);
        compare(findChild(details, "detailsTitle").text, "other-tool");
        compare(findChild(details, "detailsSubtitle").text, "apt");
        verify(findChild(details, "detailsSkeleton").visible);
        verify(!findChild(details, "detailsBody").visible);
        compare(details.idealHeight, settled);
        verify(details.metadataText.length === 0);
        verify(!details.hasDetails);
        // The pulse runs only with motion on.
        tryVerify(() => findChild(details, "detailsSkeleton").pulsing);
        App.Theme.reduceMotion = true;
        verify(!findChild(details, "detailsSkeleton").pulsing);
        compare(findChild(details, "detailsSkeleton").opacity, 1);

        // A short row settles to its own height once its details are in.
        details.detailMatchesSelection = true;
        details.detailsData = {package: details.selected};
        verify(!details.loading);
        verify(!findChild(details, "detailsSkeleton").visible);
        tryVerify(() => details.idealHeight < 130, 1000, "ideal height " + details.idealHeight);
    }

    function test_details_show_facts_a_safe_link_and_folded_dependencies() {
        App.Theme.reduceMotion = true;
        const details = createTemporaryObject(detailsComponent, test,
            {selected: row("synthetic-tool"), detailMatchesSelection: true, sourceName: (id) => id.toUpperCase()});
        details.detailsData = {publisher: "Example publisher", license: "MIT", homepage: "https://example.invalid/app",
            dependencies: ["libc6", "synthetic-library", "zlib1g"]};
        waitForRendering(details);
        compare(findChild(details, "detailsSubtitle").text, "APT");
        const metadata = findChild(details, "packageMetadata");
        verify(metadata.visible);
        verify(metadata.text.indexOf("Publisher: Example publisher") >= 0);
        compare(details.facts.length, 2);
        const link = findChild(details, "homepageLink");
        verify(link.visible);
        verify(link.enabled);
        compare(link.Accessible.role, Accessible.Link);
        // Collapsed by default, with the count in the heading.
        compare(findChild(details, "dependenciesHeading").text, "Dependencies (3)");
        verify(!findChild(details, "dependencyChips").visible);
        mouseClick(findChild(details, "dependenciesToggle"));
        verify(details.dependenciesExpanded);
        verify(findChild(details, "dependencyChips").visible);
        // A new row folds them again.
        details.selectionIdentity = "other";
        verify(!details.dependenciesExpanded);
        // Something that is not a web address is shown, not opened.
        details.detailsData = {homepage: "file:///etc/passwd"};
        verify(!findChild(details, "homepageLink").enabled);
    }

    function test_details_action_matches_the_installed_state() {
        const details = createTemporaryObject(detailsComponent, test,
            {selected: row("synthetic-tool"), detailMatchesSelection: true});
        const action = findChild(details, "detailsActionButton");
        verify(!action.visible);
        verify(!findChild(details, "installedChip").visible);
        details.installed = true;
        details.actionText = "Remove";
        details.actionSymbol = "remove";
        details.actionTone = "danger";
        verify(findChild(details, "installedChip").visible);
        verify(action.visible);
        compare(details.actionColor, App.Theme.danger);
        compare(action.Accessible.name, "Remove synthetic-tool");
        spyA.target = details;
        spyA.signalName = "actionRequested";
        waitForRendering(details);
        mouseClick(action);
        compare(spyA.count, 1);
        // A click leaves no focus ring; keyboard focus shows one.
        verify(!action.visualFocus);
        details.actionEnabled = false;
        verify(!action.enabled);
        details.compact = true;
        compare(action.text, "");
        verify(findChild(details, "detailsSourceBadge").visible);
    }

    function test_scrollbar_fades_in_while_scrolling_and_out_when_idle() {
        const list = createTemporaryObject(scrollComponent, test);
        const bar = list.Controls.ScrollBar.vertical;
        waitForRendering(list);
        verify(bar.plainHandle);
        verify(bar.size < 1);
        verify(bar.visible);
        tryCompare(bar, "revealed", false);
        compare(bar.handleOpacity, 0);
        list.contentY = 300;
        verify(bar.revealed);
        verify(bar.handleOpacity > 0);
        tryCompare(bar, "revealed", false);
        tryCompare(bar.contentItem, "opacity", 0);
        // Without motion the handle shows and hides without fading.
        App.Theme.reduceMotion = true;
        list.contentY = 600;
        verify(bar.revealed);
        compare(bar.contentItem.opacity, bar.handleOpacity);
        tryCompare(bar, "revealed", false);
        compare(bar.contentItem.opacity, 0);
    }

    function test_scroll_view_hides_the_horizontal_bar_when_content_fits() {
        const view = createTemporaryObject(scrollViewComponent, test);
        waitForRendering(view);
        verify(!view.horizontallyScrollable);
        verify(!view.Controls.ScrollBar.horizontal.visible);
        compare(view.rightPadding, App.Theme.scrollGutter);
        view.contentWidth = 600;
        verify(view.horizontallyScrollable);
        verify(view.Controls.ScrollBar.horizontal.visible);
    }

    function test_activity_explains_its_empty_state_and_hides_an_unused_header() {
        const pane = createTemporaryObject(activityComponent, test);
        waitForRendering(pane);
        verify(findChild(pane, "activityEmpty").visible);
        compare(findChild(pane, "activityEmptyTitle").text, "Nothing has run yet");
        compare(findChild(pane, "activityEmptyHint").text, "Installs, removals and updates you run will appear here.");
        verify(findChild(pane, "activityTitle").visible);
        pane.showHeader = false;
        verify(!findChild(pane, "activityTitle").visible);
        verify(!findChild(pane, "activityHeader").visible);
        verify(!findChild(pane, "cancelQueuedButton").visible);
        pane.entries = [{id: 1, state: "queued", started_at: 1, operations: [{install: {backend: "apt", name: "a"}}], outcomes: []}];
        verify(findChild(pane, "activityHeader").visible);
        verify(findChild(pane, "cancelQueuedButton").visible);
    }

    function test_activity_uses_source_names_and_expands_entries() {
        App.Theme.reduceMotion = true;
        const pane = createTemporaryObject(activityComponent, test, {sourceName: (id) => id === "apt" ? "APT" : id});
        compare(pane.target({install: {backend: "apt", name: "synthetic-tool", scope: "system"}}), "Install synthetic-tool (APT, System)");
        compare(pane.result({state: "interrupted", outcomes: []}), "Interrupted");
        pane.entries = [{id: 5, state: "finished", started_at: 100, finished_at: 190, outcomes: ["finished", "finished"],
            operations: [{install: {backend: "apt", name: "one"}}, {install: {backend: "apt", name: "two"}}],
            log: "Setting up one\nSetting up two"}];
        const list = findChild(pane, "activityList");
        tryVerify(() => list.itemAtIndex(0) !== null);
        const card = list.itemAtIndex(0);
        verify(card.canExpand);
        verify(!card.expanded);
        verify(!findChild(card, "activityDetails").visible);
        const collapsed = card.height;
        waitForRendering(pane);
        mouseClick(findChild(card, "activityEntryToggle"));
        verify(card.expanded);
        verify(findChild(card, "activityDetails").visible);
        verify(findChild(card, "activityLog").visible);
        tryVerify(() => card.height > collapsed);
        compare(pane.duration(pane.entries[0]), "1 min 30 s");
        // Expansion survives a model refresh.
        pane.entries = pane.entries.slice();
        tryVerify(() => list.itemAtIndex(0) !== null && list.itemAtIndex(0).expanded);
    }
}
