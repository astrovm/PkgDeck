import QtQuick
import QtTest
import "../../qml" as App

TestCase {
    id: test
    name: "PackageBrowser"
    when: windowShown
    width: 1100
    height: 760
    property var browser
    QtObject {
        id: fake
        property string repositories: "{}"
        property string lastRepositoryChange: ""
        function loadRepositories() {}
        function changeRepository(request) { lastRepositoryChange = request; }
        property string rows: "[]"
        property string details: "{}"
        property string status: "Ready"
        property string confirmation: ""
        property string confirmation_data: "{}"
        property string source_catalog: "[]"
        property string report_state: "{}"
        property string lastRetry: ""
        property string version: "9.9.9-test"
        property bool simulateLoading: false
        property bool busy: false
        property bool writing: false
        property bool upgradable: false
        property string lastView: ""
        property string lastQuery: ""
        property string lastSource: ""
        property bool lastForce: false
        property int selection: -1
        property int writes: 0
        property int cancels: 0
        property string lastChecked: ""
        function load(view, query, source, sudo, force) {
            lastView = view;
            lastQuery = query;
            lastSource = source;
            lastForce = !!force;
            if (simulateLoading) {
                rows = "[]";
                busy = true;
            }
        }
        function select(index) {
            selection = index;
            details = JSON.stringify({
                package: JSON.parse(rows)[index],
                description: "<b>literal metadata</b>",
                dependencies: ["synthetic-library"]
            });
        }
        function propose(action, index) {
            selection = index;
            confirmation_data = JSON.stringify({action: ({install:"Install", remove:"Remove", upgrade:"Update", "upgrade-all":"Update", clean:"Clean", "clean-all":"Clean", refresh:"Refresh"})[action] || "Apply"});
            confirmation = action + " synthetic-tool from apt, all, system";
        }
        function proposeChecked(identities) {
            lastChecked = identities;
        }
        function confirm(approved) {
            if (approved)
                writes++;
            confirmation = "";
            confirmation_data = "{}";
        }
        function cancel() {
            cancels++;
            busy = false;
        }
        function poll() {
        }
        function retrySource(view, query, source) { lastRetry = source; }
    }
    Component {
        id: window
        App.Browser {
            backend: fake
            repositoryIconSource: Qt.resolvedUrl("../../assets/" + (dark ? "github-dark.png" : "github.png"))
            logoIconSource: Qt.resolvedUrl("../../assets/logo.svg")
        }
    }
    function initTestCase() {
        Qt.application.organization = "PkgDeck-tests";
        Qt.application.domain = "example.invalid";
    }
    function init() {
        fake.repositories = "{}";
        fake.lastRepositoryChange = "";
        fake.rows = "[]";
        fake.details = "{}";
        fake.status = "Ready";
        fake.confirmation = "";
        fake.confirmation_data = "{}";
        fake.report_state = "{}";
        fake.lastRetry = "";
        fake.simulateLoading = false;
        fake.busy = false;
        fake.writing = false;
        fake.upgradable = false;
        fake.writes = 0;
        fake.cancels = 0;
        fake.lastChecked = "";
        fake.lastForce = false;
        browser = createTemporaryObject(window, test);
        verify(browser !== null);
        fake.source_catalog = JSON.stringify(browser.sourceIds.map((id) => ({source: id, summary: "Available", availability_kind: "available", capabilities: ["search", "installed", "upgrade", "clean"]})));
        browser.requestActivate();
        // Fresh checklist and column layout per test: QSettings persist
        // across tests in one run.
        browser.reduceMotion = false;
        browser.sourceSelection = "";
        browser.viewSourceFilters = ({});
        browser.sortColumn = "";
        browser.sortAscending = true;
        browser.nameWidth = 202;
        browser.versionWidth = 150;
        wait(30);
    }
    function cleanup() {
        browser.close();
    }
    // Popup content reparents to the Overlay; check draft state directly
    // when exercising several source choices in one test.
    function clickDelegate(delegate) {
        const at = delegate.mapToItem(browser.contentItem, delegate.width / 2, delegate.height / 2);
        mouseClick(browser, at.x, at.y);
    }
    function clickSourceCheck(index) {
        verify(browser.sourceCheckAt(index) !== null);
        browser.toggleDraftSource(browser.sourceIds[index]);
    }
    function populate() {
        fake.rows = JSON.stringify([
            {
                kind: "package",
                name: "synthetic-tool",
                source: "apt",
                architecture: "all",
                installed: null,
                candidate: "1",
                scope: "system",
                summary: "Synthetic package"
            },
            {
                kind: "package",
                name: "synthetic-tool",
                source: "homebrew",
                architecture: "x86_64",
                installed: "1",
                candidate: "2",
                update: "available",
                scope: {
                    environment: {
                        path: "/synthetic"
                    }
                },
                summary: "Other source"
            }
        ]);
        wait(30);
    }
    function test_refresh_retains_inactive_results_until_fresh_data_arrives() {
        populate();
        const previous = browser.items;
        fake.simulateLoading = true;
        browser.reload(true);
        compare(browser.items.length, previous.length);
        verify(browser.retainingResults);
        verify(!findChild(browser, "packageResults").enabled);
        browser.choose(0);
        compare(browser.selected, null);
        browser.propose("install");
        compare(fake.confirmation, "");
        fake.rows = JSON.stringify([previous[0]]);
        compare(browser.items.length, 1);
        verify(!browser.retainingResults);
        verify(findChild(browser, "packageResults").enabled);
        fake.busy = false;
        browser.reload(true);
        fake.busy = false; // Empty completion must discard the old snapshot.
        compare(browser.items.length, 0);
    }
    function test_motion_can_be_disabled_without_delaying_interactions() {
        const panel = findChild(browser, "detailsPanel");
        verify(!panel.visible);
        populate();
        browser.choose(0);
        verify(panel.visible);
        const panelHeight = panel.height;
        browser.choose(1);
        compare(panel.height, panelHeight);
        mouseClick(findChild(browser, "closeDetailsButton"));
        verify(!panel.visible);
        browser.choose(1);
        verify(panel.visible);
        browser.openView("Settings");
        const animations = findChild(browser, "animationsSetting");
        verify(animations.checked);
        mouseClick(animations);
        verify(!animations.checked);
        verify(browser.reduceMotion);
        verify(!browser.motionEnabled);
        compare(browser.feedbackDuration, 0);
        compare(browser.revealDuration, 0);
        compare(findChild(browser, "detailsContent").opacity, 1);
        browser.openView("Search");
        browser.choose(0);
        browser.propose("install");
        const dialog = findChild(browser, "confirmationDialog");
        tryCompare(dialog, "opened", true);
        keyClick(Qt.Key_I, Qt.AltModifier);
        compare(fake.writes, 1);
        tryCompare(dialog, "visible", false);
    }
    function test_completion_highlights_only_changed_package_identities() {
        populate();
        const rows = JSON.parse(fake.rows);
        fake.writing = true;
        fake.busy = true;
        fake.rows = "[]";
        fake.writing = false;
        fake.simulateLoading = true;
        fake.busy = false;
        tryCompare(fake, "busy", true); // Scheduled post-write reload.
        rows[0].installed = "2.0";
        fake.rows = JSON.stringify(rows);
        fake.busy = false;
        compare(browser.completedRows.length, 1);
        compare(browser.completedRows[0], browser.rowIdentity(rows[0]));
        tryCompare(browser, "completedRows", [], 2000);
    }
    function test_search_navigation_and_confirmation() {
        browser.openView("Search");
        const search = findChild(browser, "searchField");
        verify(search !== null);
        search.text = "synthetic-tool";
        search.forceActiveFocus();
        keyClick(Qt.Key_Return);
        compare(fake.lastView, "Search");
        compare(fake.lastQuery, "synthetic-tool");
        populate();
        const list = findChild(browser, "packageResults");
        list.forceActiveFocus();
        keyClick(Qt.Key_Down);
        compare(fake.selection, 0);
        const firstAction = findChild(list.itemAtIndex(0), "rowPackageAction");
        verify(firstAction.enabled);
        compare(firstAction.text, "Install");
        verify(firstAction.tooltipText.indexOf("apt") >= 0);
        waitForRendering(browser.contentItem);
        mouseClick(findChild(list.itemAtIndex(0), "rowPackageAction"));
        const dialog = findChild(browser, "confirmationDialog");
        tryCompare(dialog, "opened", true);
        compare(fake.writes, 0);
        keyClick(Qt.Key_C, Qt.AltModifier);
        tryCompare(dialog, "visible", false);
        compare(fake.writes, 0);
        waitForRendering(browser.contentItem);
        mouseClick(findChild(list.itemAtIndex(0), "rowPackageAction"));
        tryCompare(dialog, "opened", true);
        keyClick(Qt.Key_I, Qt.AltModifier);
        compare(fake.writes, 1);
        tryCompare(dialog, "visible", false);
        list.forceActiveFocus();
        keyClick(Qt.Key_Down);
        compare(fake.selection, 1);
        const action = findChild(list.itemAtIndex(1), "rowPackageAction");
        verify(action.enabled);
        compare(action.symbol, "remove");
        verify(findChild(browser, "packageDetails").text.indexOf("<b>literal metadata</b>") >= 0);
    }
    function test_views_loading_errors_and_resize() {
        for (const view of ["Search", "Installed", "Updates", "Clean", "Sources", "Settings", "About"]) {
            browser.openView(view);
            compare(browser.currentView, view);
        }
        browser.openView("Installed");
        populate();
        browser.choose(0);
        fake.busy = true;
        verify(!findChild(findChild(browser, "packageResults").itemAtIndex(0), "rowPackageAction").enabled);
        // Reads do not lock navigation or selection. Section switches go
        // through the cache path, never forced.
        browser.openView("Updates");
        compare(browser.currentView, "Updates");
        compare(fake.lastForce, false);
        browser.openView("Installed");
        compare(browser.currentView, "Installed");
        const list = findChild(browser, "packageResults");
        list.forceActiveFocus();
        keyClick(Qt.Key_Down);
        compare(fake.selection, 0);
        verify(findChild(browser, "resultsBusy").visible);
        verify(findChild(browser, "resultsCancel").visible);
        fake.writing = true;
        browser.openView("Updates");
        compare(browser.currentView, "Installed");
        browser.openView("Settings");
        compare(browser.currentView, "Settings");
        fake.writing = false;
        fake.busy = false;
        browser.openView("Installed");
        verify(findChild(browser, "packageResults").activeFocus);
        browser.width = 380;
        browser.height = 500;
        wait(30);
        verify(findChild(browser, "packageResults").width <= 380);
        verify(findChild(browser, "packageResults").height >= 78);
        browser.width = 1100;
        browser.height = 760;
    }
    function test_clean_view_actions() {
        browser.openView("Clean");
        compare(fake.lastView, "Clean");
        fake.rows = JSON.stringify([{
            kind: "cleanup", name: "Unused dependencies", display_name: "Unused dependencies",
            source: "apt", summary: "One package", cleanup_key: "autoremove",
            cleanup_kind: "orphan_dependencies", preview: "Remv synthetic-runtime [1.0]"
        }]);
        wait(20);
        const list = findChild(browser, "packageResults");
        compare(list.count, 1);
        compare(findChild(list.itemAtIndex(0), "rowPackageAction").symbol, "remove");
        mouseClick(findChild(list.itemAtIndex(0), "rowPackageAction"));
        verify(fake.confirmation.indexOf("clean") >= 0);
        const dialog = findChild(browser, "confirmationDialog");
        dialog.reject();
        tryCompare(dialog, "visible", false);
        const all = findChild(browser, "cleanAllButton");
        verify(all.visible);
        mouseClick(all);
        verify(fake.confirmation.indexOf("clean-all") >= 0);
        verify(!findChild(browser, "sourceFailureNotice").visible);
        fake.rows = "[]";
        wait(20);
        verify(!findChild(browser, "cleanAllButton").visible);
    }
    function test_cleanup_errors_are_separate_from_tasks() {
        browser.openView("Clean");
        fake.rows = JSON.stringify([
            {kind: "failure", name: "apt", source: "apt", summary: "Synthetic preview failure"},
            {kind: "cleanup", name: "Old downloads", source: "homebrew", cleanup_key: "cleanup", cleanup_kind: "cache", preview: "synthetic-cache"}
        ]);
        wait(20);
        compare(browser.viewItems.length, 1);
        compare(browser.originalIndex(0), 1);
        compare(browser.cleanupFailures.length, 1);
        verify(findChild(browser, "sourceFailureNotice").visible);
        mouseClick(findChild(browser, "sourceFailureDetails"));
        const dialog = findChild(browser, "sourceFailuresDialog");
        tryCompare(dialog, "visible", true);
        dialog.close();
        fake.rows = JSON.stringify([{kind: "failure", name: "apt", source: "apt", summary: "Synthetic preview failure"}]);
        wait(20);
        compare(browser.viewItems.length, 0);
        verify(!findChild(browser, "cleanAllButton").visible);
    }
    function test_appearance_and_search_does_not_relabel_old_results() {
        browser.openView("Installed");
        populate();
        compare(browser.items.length, 2);
        browser.openView("Search");
        compare(browser.items.length, 0);
        browser.openView("Settings");
        const appearance = findChild(browser, "appearanceSetting");
        appearance.currentIndex = 2;
        appearance.activated(2);
        compare(browser.dark, false);
        appearance.currentIndex = 1;
        appearance.activated(1);
        compare(browser.dark, true);
        appearance.currentIndex = 0;
        appearance.activated(0);
        verify(browser.systemAppearance);
        compare(browser.canvas.toString(), browser.palette.window.toString());
        verify(browser.font.pointSize > 0);
    }
    Component {
        id: vectorIcon
        Item {
            width: 48
            height: 48
            property alias name: inner.name
            property alias ink: inner.ink
            property alias drawings: inner.drawings
            property alias available: inner.available
            Rectangle {
                anchors.fill: parent
                color: "black"
            }
            App.DeckIcon {
                id: inner
                anchors.fill: parent
            }
        }
    }
    function test_bundled_vectors_render_without_an_icon_font() {
        const icon = createTemporaryObject(vectorIcon, browser.contentItem);
        verify(icon !== null);
        icon.ink = "transparent";
        waitForRendering(icon);
        const blank = grabImage(icon);
        for (const name of Object.keys(icon.drawings)) {
            icon.name = name;
            icon.ink = "#ffffff";
            waitForRendering(icon);
            verify(icon.available);
            verify(!grabImage(icon).equals(blank), name + " should paint a bundled vector");
            const light = grabImage(icon);
            icon.ink = "#102030";
            waitForRendering(icon);
            verify(!grabImage(icon).equals(light), name + " should follow the palette");
        }
    }
    function test_upgrade_all_requires_available_updates_and_confirmation() {
        browser.openView("Updates");
        const button = findChild(browser, "upgradeAllButton");
        verify(button.visible);
        verify(!button.enabled);
        populate();
        fake.upgradable = true;
        waitForRendering(browser.contentItem);
        verify(button.enabled); // No individual selection required.
        mouseClick(button);
        const dialog = findChild(browser, "confirmationDialog");
        tryCompare(dialog, "opened", true);
        verify(fake.confirmation.indexOf("upgrade-all") === 0);
        dialog.reject();
        tryCompare(dialog, "visible", false);
        compare(fake.writes, 0);
        mouseClick(button);
        tryCompare(dialog, "opened", true);
        dialog.accept();
        compare(fake.writes, 1);
        fake.busy = true;
        verify(!button.enabled);
        fake.busy = false;
        browser.openView("Installed");
        verify(!button.visible);
    }
    function test_same_app_relationship_stays_out_of_source_metadata() {
        browser.openView("Search");
        fake.rows = JSON.stringify([
            {kind: "package", name: "firefox", source: "apt", architecture: "amd64", installed: "1", candidate: "2", scope: "system", summary: "Web browser", same_app_from: ["flatpak", "snap"]},
            {kind: "package", name: "lonely", source: "apt", architecture: "amd64", installed: "1", candidate: "1", scope: "system", summary: "Only here", same_app_from: []}
        ]);
        waitForRendering(browser.contentItem);
        compare(browser.viewItems.length, 2);
        // Pooled ListView delegates reparent at the QObject level, so the
        // window-rooted search cannot descend into rows: search from the
        // results list instead.
        const results = findChild(browser, "packageResults");
        verify(results !== null);
        let line = null;
        for (let i = 0; i < 50 && line === null; i++) {
            wait(20);
            line = findChild(results, "packageSourceLine");
        }
        verify(line !== null);
        verify(line.text.indexOf("APT") >= 0);
        verify(line.text.indexOf("also in") < 0);
        compare(browser.sameAppSummary(browser.viewItems[0]), "Also installed from: Flatpak, Snap");
        compare(browser.sameAppSummary(browser.viewItems[1]), "");
    }
    function test_multi_source_toggle_lists_grouped_apps() {
        browser.openView("Installed");
        fake.rows = JSON.stringify([
            {kind: "package", name: "solo", source: "apt", architecture: "amd64", installed: "1", candidate: "1", scope: "system", summary: "Only here", same_app_from: []},
            {kind: "package", name: "duo-libs", source: "apt", architecture: "amd64", installed: "1", candidate: "2", scope: "system", summary: "In two places", same_app_from: ["homebrew"], same_app_group: "h:duo"},
            {kind: "package", name: "duo", source: "homebrew", architecture: "x86_64", installed: "2", candidate: "2", scope: "user", summary: "In two places", same_app_from: ["apt"], same_app_group: "h:duo"},
            {kind: "failure", name: "npm", source: "npm", summary: "boom", available: false}
        ]);
        waitForRendering(browser.contentItem);
        compare(browser.viewItems.length, 3);
        compare(browser.viewItems[1].groupStart, true);
        compare(browser.viewItems[1].groupTitle, "duo");
        compare(browser.viewItems[1].groupCount, 2);
        compare(browser.viewItems[1].groupSources.join(","), "APT,Homebrew");
        compare(browser.viewItems[2].groupStart, false);
        const results = findChild(browser, "packageResults");
        tryVerify(() => findChild(results, "packageGroupTitle") !== null);
        compare(findChild(results, "packageGroupTitle").text, "duo");
        const check = findChild(browser, "multiSourceCheck");
        verify(check !== null);
        verify(check.visible);
        verify(!browser.multiSourceOnly);
        compare(check.text, "Duplicate installs");
        mouseClick(check);
        compare(browser.multiSourceOnly, true);
        // The grouped app stays; failed sources are shown in the notice.
        compare(browser.viewItems.length, 2);
        compare(browser.viewItems[0].name, "duo-libs");
        compare(browser.viewItems[1].name, "duo");
        verify(findChild(browser, "sourceFailureNotice").visible);
        // Combines with the text filter.
        const field = findChild(browser, "installedFilterField");
        field.text = "duo";
        compare(browser.viewItems.length, 2);
        compare(browser.viewItems[0].name, "duo-libs");
        compare(browser.viewItems[1].name, "duo");
        field.text = "";
        mouseClick(check);
        compare(browser.multiSourceOnly, false);
        compare(browser.viewItems.length, 3);
    }
    function test_installed_view_omits_redundant_state_label() {
        browser.openView("Installed");
        const installed = {kind: "package", name: "tool", source: "apt", architecture: "amd64", installed: "1.2.3", candidate: "1.2.3", scope: "system", summary: "Tool"};
        compare(browser.versionText(installed), "1.2.3");
        browser.openView("Search");
        compare(browser.versionText(installed), "1.2.3");
        installed.candidate = "2.0.0";
        installed.update = "available";
        browser.openView("Updates");
        compare(browser.versionText(installed), "1.2.3 → 2.0.0");
    }
    function test_versionless_runtime_actions_and_rebuild_labels() {
        browser.openView("Updates");
        const runtime = {kind: "package", name: "org.example.Platform", source: "flatpak",
            architecture: "x86_64", scope: "system", installed: "", candidate: "",
            reference: "runtime/org.example.Platform/x86_64/stable", update: "available"};
        fake.rows = JSON.stringify([runtime]);
        wait(30);
        browser.choose(0);
        const action = findChild(findChild(browser, "packageResults").itemAtIndex(0), "rowPackageAction");
        verify(action.enabled);
        compare(action.symbol, "updates");
        mouseClick(action);
        compare(fake.selection, 0);
        verify(fake.confirmation.indexOf("upgrade ") === 0);
        const dialog = findChild(browser, "confirmationDialog");
        tryCompare(dialog, "opened", true);
        keyClick(Qt.Key_C, Qt.AltModifier);
        tryCompare(dialog, "visible", false);
        compare(browser.versionText(runtime), "—");
        runtime.installed = "1.0";
        runtime.candidate = "1.0";
        compare(browser.versionText(runtime), "1.0");
        runtime.installed = "";
        runtime.candidate = "2.0";
        compare(browser.versionText(runtime), "2.0");
    }
    function test_completed_write_forces_current_view_refresh() {
        browser.openView("Installed");
        populate();
        fake.lastForce = false;
        fake.writing = true;
        fake.busy = true;
        // The controller invalidates every package snapshot after a write.
        fake.rows = "[]";
        fake.busy = false;
        fake.writing = false;
        tryCompare(fake, "lastForce", true);
        compare(fake.lastView, "Installed");
        compare(fake.lastQuery, "");
    }
    function test_updates_checked_by_default_and_upgrade_subset() {
        browser.openView("Updates");
        populate();
        waitForRendering(browser.contentItem);
        // Every row starts checked, so Select all stays hidden until
        // something is deselected; the old Update selected button is gone.
        verify(findChild(browser, "upgradeSelectedButton") === null);
        compare(browser.selectedCount(), 2);
        const upgradeBtn = findChild(browser, "upgradeAllButton");
        verify(upgradeBtn.visible);
        compare(upgradeBtn.text, "Update all");
        const selectNone = findChild(browser, "selectNoneButton");
        verify(selectNone.visible);
        // Deselect one row: the single button switches to the subset path.
        browser.togglePackage(browser.items[1]);
        compare(browser.selectedCount(), 1);
        compare(browser.uncheckedPackages.length, 1);
        compare(upgradeBtn.text, "Update selected");
        mouseClick(upgradeBtn);
        verify(fake.lastChecked !== "");
        const sent = JSON.parse(fake.lastChecked);
        compare(sent.length, 1);
        // Identities mirror the controller shape: [source, name, arch, remote, scope].
        verify(sent[0].indexOf("synthetic-tool") >= 0);
        verify(sent[0].indexOf("apt") >= 0);
        // Select none hides the upgrade button; re-checking shows it again.
        mouseClick(selectNone);
        compare(browser.selectedCount(), 0);
        verify(!upgradeBtn.visible);
        // Select all re-checks everything and restores the Update all path.
        const selectAll = findChild(browser, "selectAllButton");
        verify(selectAll.visible);
        waitForRendering(browser.contentItem);
        mouseClick(selectAll);
        compare(browser.selectedCount(), 2);
        compare(browser.uncheckedPackages.length, 0);
        verify(upgradeBtn.visible);
        compare(upgradeBtn.text, "Update all");
        browser.togglePackage(browser.items[1]);
        compare(browser.selectedCount(), 1);
        verify(upgradeBtn.visible);
        compare(upgradeBtn.text, "Update selected");
    }
    function test_columns_sort_resize_and_index_mapping() {
        browser.openView("Search");
        fake.rows = JSON.stringify([
            {kind: "package", name: "bravo", source: "apt", architecture: "all", installed: null, candidate: "1", scope: "system", summary: "B"},
            {kind: "package", name: "alpha", source: "apt", architecture: "all", installed: "1", candidate: "2", scope: "system", summary: "A"}
        ]);
        waitForRendering(browser.contentItem);
        compare(browser.viewItems[0].name, "bravo");
        const header0 = findChild(browser, "columnHeader0");
        verify(header0 !== null);
        mouseClick(header0);
        compare(browser.viewItems[0].name, "alpha");
        verify(header0.text.indexOf("▲") >= 0);
        // The visible index maps back to backend order for actions.
        browser.choose(0);
        compare(fake.selection, 1);
        mouseClick(header0);
        compare(browser.viewItems[0].name, "bravo");
        verify(header0.text.indexOf("▼") >= 0);
        browser.choose(0);
        compare(fake.selection, 0);
        const grip = findChild(browser, "columnResize0");
        verify(grip !== null);
        compare(browser.nameWidth, 202);
        mousePress(grip, grip.width / 2, grip.height / 2);
        mouseMove(grip, grip.width / 2 + 60, grip.height / 2);
        mouseRelease(grip, grip.width / 2 + 60, grip.height / 2);
        verify(browser.nameWidth > 202);
        // Keyboard sorting through the header Tab stop (currently descending).
        const sortArea = header0.children[0];
        sortArea.forceActiveFocus();
        verify(sortArea.activeFocus);
        keyClick(Qt.Key_Space);
        compare(browser.viewItems[0].name, "alpha");
        verify(header0.text.indexOf("▲") >= 0);
    }
    function test_repository_sidebar() {
        compare(browser.repositoryUrl.toString(), "https://github.com/astrovm/PkgDeck");
        const icon = findChild(browser, "repositoryIcon");
        tryCompare(icon, "status", Image.Ready);
        const link = findChild(browser, "repositoryLink");
        verify(link.visible);
        link.forceActiveFocus();
        verify(link.activeFocus);
        browser.width = 380;
        browser.height = 500;
        wait(30);
        verify(!link.visible); // Attribution belongs to the sidebar, not a window footer.
        browser.width = 1100;
        wait(30);
        verify(link.visible);
        verify(link.width >= 24);
    }
    function test_about_shows_backend_version() {
        browser.openView("About");
        compare(browser.currentView, "About");
        const about = findChild(browser, "aboutText");
        verify(about !== null);
        verify(about.text.indexOf("9.9.9-test") >= 0);
        verify(about.text.indexOf("Ctrl+2: Installed") >= 0);
        const at = about.mapToItem(browser.contentItem, 0, 0);
        verify(at.y < browser.height / 3);
        // A tall window must not vertically center the text (regression:
        // the label used to float mid-window without a fill-height item).
        browser.height = 1300;
        waitForRendering(browser.contentItem);
        const tall = about.mapToItem(browser.contentItem, 0, 0);
        verify(tall.y < browser.height / 3);
        browser.width = 360;
        browser.height = 400;
        waitForRendering(browser.contentItem);
        const scroll = findChild(browser, "aboutScroll");
        verify(about.width <= scroll.availableWidth);
        if (scroll.contentHeight > scroll.availableHeight) {
            scroll.contentItem.contentY = scroll.contentHeight - scroll.availableHeight;
            verify(scroll.contentItem.contentY > 0);
        }
        verify(!findChild(browser, "sourceFilter").visible);
    }
    function test_view_status_and_source_columns() {
        browser.openView("Settings");
        verify(findChild(browser, "appearanceSetting") !== null);
        verify(findChild(browser, "authorizationSetting") !== null);
        browser.openView("About");
        verify(findChild(browser, "aboutText").visible);
        browser.openView("Sources");
        compare(findChild(browser, "columnHeader0").text, "SOURCE");
        compare(findChild(browser, "columnHeader1").text, "STATUS");
        compare(findChild(browser, "columnHeader2").text, "CAPABILITIES");
        browser.openView("Search");
        compare(findChild(browser, "columnHeader0").text, "NAME / SOURCE");
        compare(findChild(browser, "columnHeader1").text, "VERSION");
        compare(findChild(browser, "columnHeader2").text, "SUMMARY");
        browser.reload();
        fake.rows = JSON.stringify([
            {
                kind: "source",
                name: "apt",
                source: "apt",
                summary: "Available",
                available: true,
                capabilities: ["search", "installed"]
            }
        ]);
        wait(30);
        compare(browser.items.length, 1);
        compare(browser.items[0].capabilities.join(","), "search,installed");
    }
    function test_search_focus_and_list_keys() {
        browser.openView("Search");
        populate();
        const search = findChild(browser, "searchField");
        const list = findChild(browser, "packageResults");
        verify(search.activeFocus);
        search.forceActiveFocus();
        search.text = "synthetic";
        fake.rows = JSON.stringify(JSON.parse(fake.rows).reverse());
        wait(30);
        verify(search.activeFocus);
        verify(!list.activeFocus);
        compare(browser.queryDirty, true);
        fake.busy = true;
        fake.rows = JSON.stringify(JSON.parse(fake.rows).reverse());
        fake.rows = JSON.stringify(JSON.parse(fake.rows).reverse());
        fake.busy = false;
        wait(30);
        verify(search.activeFocus); // Every partial and completion preserve typing.
        // Best-match ranking puts the apt row first visibly although the
        // backend order is reversed; selection follows the visible order.
        keyClick(Qt.Key_Down);
        compare(fake.selection, 1);
        keyClick(Qt.Key_PageDown);
        compare(fake.selection, 0);
        keyClick(Qt.Key_Home);
        compare(fake.selection, 1);
        keyClick(Qt.Key_End);
        compare(fake.selection, 0);
    }
    function test_ctrl_f_uses_current_view_and_restores_focus() {
        browser.openView("Installed");
        const installed = findChild(browser, "installedFilterField");
        installed.text = "anonymous";
        keyClick(Qt.Key_F, Qt.ControlModifier);
        verify(installed.activeFocus);
        browser.openView("Updates");
        const list = findChild(browser, "packageResults");
        list.forceActiveFocus();
        keyClick(Qt.Key_F, Qt.ControlModifier);
        const popup = findChild(browser, "sourcePopup");
        tryCompare(popup, "visible", true);
        tryVerify(() => findChild(browser, "sourcePickerSearch").activeFocus);
        popup.close();
        tryVerify(() => list.activeFocus);
        browser.openView("Search");
        keyClick(Qt.Key_F, Qt.ControlModifier);
        verify(findChild(browser, "searchField").activeFocus);
    }
    function test_installed_filter_keeps_focus_during_streaming() {
        browser.openView("Installed");
        populate();
        browser.choose(0);
        const filter = findChild(browser, "installedFilterField");
        filter.forceActiveFocus();
        filter.text = "synthetic";
        fake.busy = true;
        fake.rows = JSON.stringify(JSON.parse(fake.rows).reverse());
        fake.busy = false;
        wait(30);
        verify(filter.activeFocus);
    }
    function test_update_selection_cache_tracks_rows_and_deselections() {
        browser.openView("Updates");
        populate();
        compare(browser.selectedCount(), 2);
        browser.selectNonePackages();
        compare(browser.selectedCount(), 0);
        const rows = JSON.parse(fake.rows);
        rows.push(Object.assign({}, rows[0], {name: "new-update"}));
        fake.rows = JSON.stringify(rows);
        compare(browser.selectedCount(), 1);
        verify(browser.packageChecked(rows[2]));
        browser.uncheckedPackages = [];
        compare(browser.selectedCount(), 3);
    }
    function test_search_ranks_best_matches_first() {
        browser.openView("Search");
        const search = findChild(browser, "searchField");
        search.text = "fire";
        fake.rows = JSON.stringify([
            {kind: "package", name: "x-fire-helper", source: "apt", architecture: "all", installed: null, candidate: "1", scope: "system", summary: "Helper"},
            {kind: "package", name: "firefox", source: "apt", architecture: "all", installed: null, candidate: "1", scope: "system", summary: "Browser"},
            {kind: "package", name: "fire", source: "apt", architecture: "all", installed: null, candidate: "1", scope: "system", summary: "Exact"},
            {kind: "package", name: "zzz", source: "apt", architecture: "all", installed: null, candidate: "1", scope: "system", summary: "Fire starter"},
            {kind: "package", name: "fire", source: "npm", architecture: "x64", installed: null, candidate: null, scope: "system", summary: "Install fire with npm"},
            {kind: "failure", name: "bun", source: "bun", summary: "boom", available: false}
        ]);
        waitForRendering(browser.contentItem);
        compare(browser.viewItems.length, 4);
        compare(browser.viewItems[0].name, "fire");
        compare(browser.viewItems[0].source, "apt");
        compare(browser.viewItems[1].name, "firefox");
        compare(browser.viewItems[2].name, "x-fire-helper");
        compare(browser.viewItems[3].name, "zzz");
        // Failed sources are separate; guessed offers are excluded entirely.
        verify(findChild(browser, "sourceFailureNotice").visible);
        browser.choose(0);
        compare(fake.selection, 2);
        browser.cycleSort("name");
        compare(browser.viewItems[0].name, "fire");
        compare(browser.viewItems[0].source, "apt");
        compare(browser.viewItems[3].name, "zzz");
        // Submitting a fresh search resets to best-match order and forces
        // a native query instead of serving the cached snapshot.
        search.forceActiveFocus();
        keyClick(Qt.Key_Return);
        compare(browser.sortColumn, "");
        compare(browser.viewItems[0].name, "fire");
        compare(fake.lastForce, true);
        compare(fake.lastView, "Search");
    }
    function test_search_compares_sources_and_actions_without_opening_details() {
        const field = findChild(browser, "searchField");
        field.text = "player";
        fake.rows = JSON.stringify([
            {kind: "package", name: "player-plugin", source: "apt", installed: null, candidate: "1", scope: "system"},
            {kind: "package", name: "org.example.Player", source: "flatpak", installed: null, candidate: "1", scope: "system"},
            {kind: "package", name: "player", source: "apt", installed: "1", candidate: "1", scope: "system"},
            {kind: "package", name: "player", source: "snap", installed: null, candidate: "1", scope: "system"},
            {kind: "package", name: "player", source: "npm", installed: null, candidate: null, scope: "system"}
        ]);
        waitForRendering(browser.contentItem);
        compare(browser.viewItems.length, 4);
        compare(browser.viewItems[3].name, "player-plugin");
        const list = findChild(browser, "packageResults");
        for (let i = 0; i < 3; ++i) {
            const row = browser.viewItems[i];
            const action = findChild(list.itemAtIndex(i), "rowPackageAction");
            verify(action !== null);
            compare(action.symbol, row.installed ? "remove" : "install");
            mouseClick(action);
            compare(fake.selection, browser.originalIndex(i));
            verify(fake.confirmation.indexOf(row.installed ? "remove" : "install") === 0);
            verify(browser.selected === null);
            const dialog = findChild(browser, "confirmationDialog");
            tryCompare(dialog, "opened", true);
            keyClick(Qt.Key_C, Qt.AltModifier);
            tryCompare(dialog, "visible", false);
        }
        compare(fake.writes, 0);
    }
    function test_package_icon_loads_and_falls_back_on_error() {
        populate();
        const rows = JSON.parse(fake.rows);
        rows[0].icon = Qt.resolvedUrl("../../assets/logo.svg").toString().replace("file://", "");
        fake.rows = JSON.stringify(rows);
        const results = findChild(browser, "packageResults");
        waitForRendering(browser.contentItem);
        let row = results.itemAtIndex(0);
        tryCompare(findChild(row, "packageIcon"), "visible", true);
        verify(!findChild(row, "packageIconFallback").visible);
        rows[0].icon = "/missing/synthetic-icon.png";
        fake.rows = JSON.stringify(rows);
        waitForRendering(browser.contentItem);
        row = results.itemAtIndex(0);
        tryCompare(findChild(row, "packageIconFallback"), "visible", true);
        verify(!findChild(row, "packageIcon").visible);
    }
    function test_flatpak_installation_scopes_are_visible_and_selectable() {
        populate();
        const app = {kind: "package", name: "org.example.Player", display_name: "Player",
            source: "flatpak", remote: "flathub", architecture: "x86_64", candidate: "1",
            reference: "org.example.Player/x86_64/stable", installed: null, update: "unknown"};
        fake.rows = JSON.stringify([
            Object.assign({}, app, {scope: {user: {uid: 1000}}}),
            Object.assign({}, app, {scope: "system"})
        ]);
        const results = findChild(browser, "packageResults");
        tryCompare(results, "count", 2);
        waitForRendering(browser.contentItem);
        const userRow = results.itemAtIndex(0);
        const systemRow = results.itemAtIndex(1);
        compare(findChild(userRow, "packageSourceLine").text, "FLATPAK · flathub · User");
        compare(findChild(systemRow, "packageSourceLine").text, "FLATPAK · flathub · System");
        mouseClick(findChild(systemRow, "rowPackageAction"));
        compare(fake.selection, 1);
        const dialog = findChild(browser, "confirmationDialog");
        tryCompare(dialog, "opened", true);
        keyClick(Qt.Key_C, Qt.AltModifier);
        tryCompare(dialog, "visible", false);
        compare(fake.writes, 0);
    }
    function test_failed_screenshots_collapse_and_keep_working_images() {
        populate();
        browser.choose(0);
        const row = JSON.parse(fake.rows)[0];
        const good = Qt.resolvedUrl("../../assets/logo.svg").toString();
        const bad = Qt.resolvedUrl("missing-synthetic-screenshot.png").toString();
        fake.details = JSON.stringify({package: row, screenshots: [{url: bad}, {url: good}]});
        const gallery = findChild(browser, "screenshotGallery");
        tryCompare(gallery, "count", 1);
        compare(browser.visibleScreenshots[0].url, good);
        verify(gallery.visible);
        fake.details = JSON.stringify({package: row, screenshots: [{url: bad}]});
        tryCompare(gallery, "count", 0);
        verify(!gallery.visible);
        // Reopening details may retry images after a transient network failure.
        browser.choose(1);
        browser.choose(0);
        fake.details = JSON.stringify({package: row, screenshots: [{url: good}]});
        tryCompare(gallery, "count", 1);
        verify(gallery.visible);
    }
    function test_compact_action_content_is_centered_and_inside_button() {
        populate();
        fake.busy = true;
        const button = findChild(browser, "resultsCancel");
        tryCompare(button, "visible", true);
        waitForRendering(browser.contentItem);
        const contents = button.contentItem.children[0];
        const position = contents.mapToItem(button, 0, 0);
        verify(position.y >= 0);
        verify(position.y + contents.height <= button.height);
        verify(Math.abs(position.y + contents.height / 2 - button.height / 2) <= 1);
        verify(Math.abs(position.x + contents.width / 2 - button.width / 2) <= 1);
    }
    function test_app_screenshots_follow_selected_identity_and_open_viewer() {
        populate();
        browser.choose(0);
        const gallery = findChild(browser, "screenshotGallery");
        verify(!gallery.visible);
        const row = JSON.parse(fake.rows)[0];
        fake.details = JSON.stringify({package: row, description: "Example app", screenshots: [
            {url: Qt.resolvedUrl("../../assets/logo.svg").toString(), caption: "Example screenshot"}
        ]});
        tryCompare(gallery, "visible", true);
        tryCompare(gallery, "count", 1);
        waitForRendering(browser.contentItem);
        const thumbnail = gallery.itemAtIndex(0);
        verify(thumbnail !== null);
        tryCompare(thumbnail, "enabled", true);
        browser.width = 400;
        browser.height = 520;
        waitForRendering(browser.contentItem);
        const panel = findChild(browser, "detailsPanel");
        const galleryPosition = gallery.mapToItem(panel, 0, 0);
        verify(galleryPosition.y + gallery.height <= panel.height - 12);
        const actions = findChild(browser, "updatesActions");
        const actionsPosition = actions.mapToItem(browser.contentItem, 0, 0);
        verify(actionsPosition.y + actions.height <= browser.contentItem.height);
        mouseClick(thumbnail);
        const viewer = findChild(browser, "screenshotDialog");
        tryCompare(viewer, "opened", true);
        compare(viewer.title, "Example screenshot");
        verify(browser.screenshotUrl.length > 0);
        viewer.close();
        tryCompare(viewer, "visible", false);
        compare(browser.screenshotUrl, "");
        browser.choose(1);
        // A late details response from another identity must never show its images.
        fake.details = JSON.stringify({package: row, screenshots: [{url: "https://example.invalid/stale.png"}]});
        compare(gallery.count, 0);
        verify(!gallery.visible);
        const close = findChild(browser, "closeDetailsButton");
        compare(close.text, "");
        mouseClick(close);
        verify(browser.selected === null);
        verify(!findChild(browser, "detailsPanel").visible);
        compare(fake.writes, 0);
    }
    function test_repository_scopes_and_actions() {
        browser.openView("Sources");
        const rows = [
            {backend: "flatpak", name: "fixture", title: "Fixture", url: "https://example.invalid", scope: {user: {uid: 1000}}, enabled: true, priority: 1},
            {backend: "flatpak", name: "fixture", title: "Fixture", url: "https://example.invalid", scope: "system", enabled: false, priority: 2}
        ];
        fake.repositories = JSON.stringify({repositories: rows, errors: []});
        waitForRendering(browser.contentItem);
        clickDelegate(findChild(browser, "repositoriesButton"));
        const dialog = findChild(browser, "repositoriesDialog");
        tryCompare(dialog, "visible", true);
        const list = findChild(browser, "repositoryList");
        tryCompare(list, "count", 2);
        tryVerify(() => list.itemAtIndex(1) !== null);
        clickDelegate(findChild(list.itemAtIndex(1), "repositoryEnabled"));
        let request = JSON.parse(fake.lastRepositoryChange);
        compare(request.scope, "system");
        compare(request.action, "set_enabled");
        compare(request.enabled, true);
        clickDelegate(findChild(list.itemAtIndex(0), "removeRepositoryButton"));
        request = JSON.parse(fake.lastRepositoryChange);
        compare(request.scope.user.uid, 1000);
        compare(request.action, "remove");
        compare(fake.writes, 0);
        dialog.close();
        browser.repositoryChange({backend: "flatpak", name: "new", scope: "system"}, "add", {url: "https://example.invalid/new.flatpakrepo"});
        request = JSON.parse(fake.lastRepositoryChange);
        compare(request.url, "https://example.invalid/new.flatpakrepo");
        compare(request.scope, "system");
    }
    function test_firmware_row_updates_instead_of_removing() {
        browser.openView("Installed");
        fake.rows = JSON.stringify([{kind: "package", name: "synthetic-device", display_name: "Synthetic BIOS", source: "fwupd", architecture: "device", installed: "1", candidate: "2", update: "available", scope: "system", summary: "Firmware · AC power required"}]);
        const list = findChild(browser, "packageResults");
        tryVerify(() => list.itemAtIndex(0) !== null);
        const action = findChild(list.itemAtIndex(0), "rowPackageAction");
        compare(action.symbol, "updates");
        waitForRendering(browser.contentItem);
        clickDelegate(action);
        verify(fake.confirmation.indexOf("upgrade") === 0);
        compare(fake.writes, 0);
    }
    function test_standalone_rows_offer_only_updates() {
        browser.openView("Installed");
        fake.rows = JSON.stringify([{kind: "package", name: "codex", display_name: "Codex", source: "codex", architecture: "x86_64", installed: "1.0.0", candidate: "2.0.0", update: "available", scope: {user: {uid: 1000}}, summary: "Standalone CLI"}]);
        const list = findChild(browser, "packageResults");
        tryVerify(() => list.itemAtIndex(0) !== null);
        const action = findChild(list.itemAtIndex(0), "rowPackageAction");
        compare(action.symbol, "updates");
        waitForRendering(browser.contentItem);
        clickDelegate(action);
        verify(fake.confirmation.indexOf("upgrade") === 0);
        fake.confirmation = "";
        fake.rows = JSON.stringify([{kind: "package", name: "codex", display_name: "Codex", source: "codex", installed: "2.0.0", candidate: "2.0.0", update: "current", scope: {user: {uid: 1000}}}]);
        tryVerify(() => list.itemAtIndex(0) !== null);
        tryVerify(() => !findChild(list.itemAtIndex(0), "rowPackageAction").visible);
        compare(fake.writes, 0);
    }
    function test_container_rows_offer_separate_pull_and_cleanup_actions() {
        browser.openView("Installed");
        fake.rows = JSON.stringify([
            {kind: "package", name: "sha256:0123456789abcdef", display_name: "example/app:latest", source: "docker", architecture: "x86_64", installed: "0123456789ab", candidate: null, update: "unknown", scope: "system", remote: "Docker daemon", reference: "example/app:latest", summary: "Tags: example/app:latest · 42MB"},
            {kind: "package", name: "fedcba9876543210", display_name: "Untagged image fedcba987654", source: "podman", architecture: "x86_64", installed: "fedcba987654", candidate: null, update: "unknown", scope: {user: {uid: 1000}}, remote: "rootless Podman storage", reference: null, summary: "Dangling · 9MB"}
        ]);
        const list = findChild(browser, "packageResults");
        tryVerify(() => list.itemAtIndex(1) !== null);
        const tagged = list.itemAtIndex(0);
        const pull = findChild(tagged, "rowContainerPull");
        const remove = findChild(tagged, "rowPackageAction");
        verify(pull.visible);
        compare(pull.symbol, "updates");
        compare(remove.symbol, "remove");
        clickDelegate(pull);
        verify(fake.confirmation.indexOf("upgrade") === 0);
        fake.confirmation = "";
        const dangling = list.itemAtIndex(1);
        verify(!findChild(dangling, "rowContainerPull").visible);
        compare(findChild(dangling, "rowPackageAction").symbol, "remove");
        compare(fake.writes, 0);
    }
    function test_header_source_checklist_and_installed_filter() {
        browser.openView("Installed");
        const filter = findChild(browser, "sourceFilter");
        verify(filter !== null);
        compare(filter.text, "Available sources");
        const popup = findChild(browser, "sourcePopup");
        verify(popup !== null);
        popup.open();
        tryCompare(popup, "visible", true);
        const npm = browser.sourceIds.indexOf("npm");
        verify(browser.sourceCheckAt(npm).checked);
        clickSourceCheck(npm);
        verify(popup.draftSources.indexOf("npm") < 0);
        compare(browser.sourceSelection, "");
        compare(fake.lastSource, "");
        browser.applySourceDraft();
        compare(filter.text, "24 sources");
        verify(fake.lastSource.split(",").indexOf("npm") < 0);
        compare(fake.lastView, "Installed");
        popup.open();
        tryCompare(popup, "visible", true);
        mouseClick(findChild(browser, "sourcePickerSearch"));
        findChild(browser, "sourcePickerSearch").text = "npm";
        verify(browser.sourceCheckAt(npm) !== null);
        findChild(browser, "sourcePickerSearch").text = "";
        browser.applySourceDraft();
        browser.viewSourceFilters = ({});
        compare(browser.sourceSelection, "");
        compare(filter.text, "Available sources");
        const field = findChild(browser, "installedFilterField");
        verify(field !== null);
        browser.openView("Installed");
        populate();
        compare(fake.lastQuery, "");
        compare(browser.viewItems.length, 2);
        // Typing narrows the loaded rows immediately without a native query.
        field.forceActiveFocus();
        field.text = "homebrew";
        compare(browser.installedFilter, "homebrew");
        compare(browser.viewItems.length, 1);
        compare(browser.viewItems[0].source, "homebrew");
        compare(fake.lastQuery, "");
        // Enter jumps to the first match; clearing restores every row.
        keyClick(Qt.Key_Return);
        compare(fake.selection, 1);
        field.text = "";
        compare(browser.viewItems.length, 2);
    }
    function test_failure_rows_show_diagnostics() {
        fake.rows = JSON.stringify([{kind: "failure", name: "npm", source: "npm", summary: "Source failed"}]);
        wait(30);
        compare(browser.viewItems.length, 0);
        verify(findChild(browser, "sourceFailureNotice").visible);
        mouseClick(findChild(browser, "sourceFailureDetails"));
        const dialog = findChild(browser, "sourceFailuresDialog");
        tryCompare(dialog, "visible", true);
        compare(browser.copyableDiagnostics(), "View: Search\nState: unknown\nnpm: failed");
        browser.retryFailedSource("npm");
        compare(fake.lastRetry, "npm");
    }
    function test_sidebar_shows_app_logo() {
        const logo = findChild(browser, "appLogo");
        verify(logo !== null);
        verify(logo.source.toString().indexOf("logo.svg") >= 0);
        tryCompare(logo, "status", Image.Ready);
    }
    function test_upgrade_all_hint() {
        browser.openView("Updates");
        fake.rows = JSON.stringify([
            {kind: "package", name: "tool", source: "apt", architecture: "all", installed: "1", candidate: "2", update: "available", summary: "Updatable"},
            {kind: "failure", name: "npm", source: "npm", summary: "boom", available: false}
        ]);
        wait(30);
        const hint = findChild(browser, "upgradeAllHint");
        verify(hint.visible);
        verify(hint.text.indexOf("source has failed") >= 0);
        verify(!findChild(browser, "upgradeAllButton").enabled);
    }
    function test_selection_survives_streaming_partials() {
        browser.openView("Search");
        populate();
        const list = findChild(browser, "packageResults");
        list.forceActiveFocus();
        keyClick(Qt.Key_Down);
        compare(fake.selection, 0);
        // A partial inserting a row above follows the identity, not the index,
        // and never refires selection on its own.
        fake.rows = JSON.stringify([
            {
                kind: "package",
                name: "aaa-first",
                source: "apt",
                architecture: "all",
                installed: null,
                candidate: "1",
                summary: "Inserted above"
            },
            ...JSON.parse(fake.rows)
        ]);
        wait(30);
        compare(list.currentIndex, 1);
        compare(fake.selection, 0);
    }
    function test_source_filter_popup_lists_sources() {
        browser.openView("Search");
        const popup = findChild(browser, "sourcePopup");
        verify(popup !== null);
        popup.open();
        tryCompare(popup, "visible", true);
        verify(browser.pickerItems().length > 0);
        const npmCheck = browser.sourceCheckAt(browser.sourceIds.indexOf("npm"));
        verify(npmCheck !== null);
        verify(npmCheck.checked);
        popup.close();
        tryCompare(popup, "visible", false);
    }
    function test_read_states_keep_success_wording_scoped() {
        browser.openView("Updates");
        fake.rows = "[]";
        fake.report_state = JSON.stringify({phase: "loading", failures: []});
        compare(browser.emptyStateMessage(), "Checking sources…");
        fake.report_state = JSON.stringify({phase: "complete", failures: []});
        compare(browser.emptyStateMessage(), "You're up to date");
        fake.report_state = JSON.stringify({phase: "partial", failures: [{source: "npm", kind: "locked", detail: "Synthetic package lock"}]});
        verify(browser.emptyStateMessage().indexOf("completed") >= 0);
        verify(browser.emptyStateMessage() !== "You're up to date");
        compare(browser.copyableDiagnostics(), "View: Updates\nState: partial\nnpm: locked");
        fake.report_state = JSON.stringify({phase: "failed", failures: [{source: "apt", kind: "authorization"}]});
        compare(browser.emptyStateMessage(), "Could not check these sources.");
        fake.report_state = JSON.stringify({phase: "cached", failures: []});
        compare(browser.emptyStateMessage(), "No results in the last check.");
        fake.report_state = JSON.stringify({phase: "complete", failures: []});
        fake.rows = JSON.stringify([{kind: "package", name: "anonymous", source: "homebrew", installed: "1", candidate: "2", update: "available"}]);
        browser.viewSourceFilters = ({Updates: ["apt"]});
        compare(browser.viewItems.length, 0);
        compare(browser.emptyStateMessage(), "No results from selected sources.");
        compare(browser.sourceSummary(), "APT");
    }
    function test_manager_settings_are_separate_from_page_filter() {
        browser.openView("Settings");
        const popup = findChild(browser, "sourcePopup");
        popup.mode = "settings";
        popup.open();
        tryCompare(popup, "visible", true);
        browser.toggleDraftSource("npm");
        browser.applySourceDraft();
        verify(browser.sourceSelection.split(",").indexOf("npm") < 0);
        compare(browser.viewSourceFilters["Installed"], undefined);
        browser.openView("Installed");
        verify(fake.lastSource.split(",").indexOf("npm") < 0);
        popup.mode = "filter";
        popup.open();
        tryCompare(popup, "visible", true);
        browser.toggleDraftSource("apt");
        browser.applySourceDraft();
        verify(browser.sourceSelection.split(",").indexOf("npm") < 0);
        verify(browser.viewSourceFilters["Installed"] !== undefined);
    }
    function test_source_picker_uses_discovered_capabilities() {
        fake.source_catalog = JSON.stringify([
            {source: "apt", summary: "Available", availability_kind: "available", capabilities: ["search", "installed", "upgrade"]},
            {source: "npm", summary: "Manager missing", availability_kind: "unavailable", capabilities: ["search", "installed"]}
        ]);
        browser.openView("Updates");
        const popup = findChild(browser, "sourcePopup");
        popup.mode = "filter";
        popup.open();
        tryCompare(popup, "visible", true);
        compare(browser.pickerItems().join(","), "apt");
        popup.showUnavailable = true;
        verify(browser.pickerItems().indexOf("npm") >= 0);
        verify(!browser.sourceCheckAt(browser.sourceIds.indexOf("npm")).enabled);
        popup.close();
        popup.mode = "settings";
        popup.open();
        tryCompare(popup, "visible", true);
        verify(browser.pickerItems().indexOf("npm") >= 0);
    }
    function test_close_requests_cancellation() {
        fake.busy = true;
        browser.close();
        compare(fake.cancels, 1);
    }
}
