import QtQuick
import QtQuick.Controls as Controls
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
        property string progress: "{}"
        property string confirmation: ""
        property string confirmation_data: "{}"
        property string source_catalog: "[]"
        property int sourceChecks: 0
        function checkSources() { sourceChecks++; }
        property string report_state: "{}"
        property string manifest_preview: "{}"
        property string lastInventorySelection: ""
        function exportInventory(url, identities) { lastInventorySelection = identities; }
        function previewInventory(url) {}
        property string activity: "[]"
        property string background_state: "{}"
        property string lastRetry: ""
        property string version: "9.9.9-test"
        property bool simulateLoading: false
        property bool simulateOpening: false
        property bool busy: false
        property bool writing: false
        property bool upgradable: false
        property string lastView: ""
        property string lastQuery: ""
        property string lastSource: ""
        property int loadCount: 0
        property bool lastForce: false
        property int selection: -1
        property int writes: 0
        property int cancels: 0
        property string lastChecked: ""
        property string lastOpenedInput: ""
        function openInput(input) { lastOpenedInput = input; if (simulateOpening) busy = true; }
        function load(view, query, source, sudo, force) {
            loadCount++;
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
        function refreshActivity() {}
        function cancelQueued() {}
        function checkUpdates(sources, enabled, offline, metered, force) {}
        function setAutostart(enabled) { return true; }
    }
    Component {
        id: window
        App.Browser {
            backend: fake
            logoIconSource: Qt.resolvedUrl("../../assets/logo.svg")
            repositoryIconSource: Qt.resolvedUrl("../../assets/github.svg")
        }
    }
    function initTestCase() {
        Qt.application.organization = "PkgDeck-tests";
        Qt.application.domain = "example.invalid";
    }
    function init() {
        fake.repositories = "{}";
        fake.lastRepositoryChange = "";
        fake.sourceChecks = 0;
        fake.rows = "[]";
        fake.details = "{}";
        fake.status = "Ready";
        fake.progress = "{}";
        fake.confirmation = "";
        fake.confirmation_data = "{}";
        fake.report_state = "{}";
        fake.activity = "[]";
        fake.background_state = "{}";
        fake.lastRetry = "";
        fake.manifest_preview = "{}";
        fake.lastInventorySelection = "";
        fake.simulateLoading = false;
        fake.simulateOpening = false;
        fake.busy = false;
        fake.writing = false;
        fake.upgradable = false;
        fake.writes = 0;
        fake.cancels = 0;
        fake.lastChecked = "";
        fake.lastOpenedInput = "";
        fake.lastForce = false;
        fake.loadCount = 0;
        browser = createTemporaryObject(window, test);
        verify(browser !== null);
        fake.source_catalog = JSON.stringify(browser.sourceIds.map((id) => ({source: id, summary: "Available", availability_kind: "available", capabilities: ["search", "installed", "upgrade", "clean"]})));
        browser.requestActivate();
        // Fresh checklist and column layout per test: QSettings persist
        // across tests in one run.
        browser.reduceMotion = false;
        browser.backgroundMode = false;
        browser.autostartEnabled = false;
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
    function test_row_actions_stay_steady_during_refresh() {
        populate();
        const rows = fake.rows;
        const list = findChild(browser, "packageResults");
        const action = findChild(list.itemAtIndex(0), "rowPackageAction");
        compare(action.text, "");
        compare(action.width, 38);
        verify(action.tooltipText.indexOf("Install") >= 0);
        fake.simulateLoading = true;
        browser.reload(true);
        verify(!action.enabled);
        compare(list.opacity, 1);
        compare(action.opacity, 1);
        fake.rows = rows;
        waitForRendering(browser.contentItem);
        compare(list.opacity, 1);
        compare(findChild(list.itemAtIndex(0), "rowPackageAction").opacity, 1);
        fake.busy = false;
        compare(findChild(list.itemAtIndex(0), "rowPackageAction").opacity, 1);
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
        waitForRendering(browser.contentItem);
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
        compare(firstAction.text, "");
        compare(firstAction.width, 38);
        verify(firstAction.tooltipText.indexOf("Install") >= 0);
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
        for (const view of ["Search", "Installed", "Updates", "Clean", "Sources", "Settings"]) {
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
        compare(browser.currentView, "Updates");
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
        verify(!all.visible);
        fake.rows = JSON.stringify([
            {kind: "cleanup", name: "Unused dependencies", source: "apt", summary: "One package", cleanup_key: "autoremove"},
            {kind: "cleanup", name: "Cached downloads", source: "apt", summary: "Two files", cleanup_key: "autoclean"}
        ]);
        wait(20);
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
        fake.simulateLoading = true;
        browser.openView("Search");
        compare(browser.items.length, 0);
        browser.openView("Settings");
        const appearance = findChild(browser, "appearanceSetting");
        verify(typeof appearance.contentItem.positionToRectangle === "function");
        verify(typeof findChild(browser, "authorizationSetting").contentItem.positionToRectangle === "function");
        waitForRendering(browser.contentItem);
        mouseClick(appearance);
        tryCompare(appearance.popup, "visible", true);
        appearance.popup.close();
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
        verify(!button.visible);
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
        compare(upgradeBtn.text, "Update selected (1)");
        mouseClick(upgradeBtn);
        verify(fake.lastChecked !== "");
        const sent = JSON.parse(fake.lastChecked);
        compare(sent.length, 1);
        // The controller expects identity arrays, not JSON strings.
        verify(Array.isArray(sent[0]));
        compare(sent[0][0], "apt");
        compare(sent[0][1], "synthetic-tool");
        compare(sent[0][2], "all");
        compare(sent[0][3], null);
        compare(sent[0][4], "system");
        compare(sent[0][5], null);
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
        compare(upgradeBtn.text, "Update selected (1)");
    }
    function test_update_selected_flatpak_keeps_its_exact_identity() {
        browser.openView("Updates");
        const reference = "app/io.github.astrovm.AdventureMods/x86_64/master";
        fake.rows = JSON.stringify([
            {kind: "package", name: "io.github.astrovm.AdventureMods", source: "flatpak",
                architecture: "x86_64", remote: "astrovm", reference: reference,
                scope: "system", installed: "0.3.14", candidate: "", update: "available"},
            {kind: "package", name: "synthetic-tool", source: "apt", architecture: "all",
                scope: "system", installed: "1", candidate: "2", update: "available"}
        ]);
        waitForRendering(browser.contentItem);
        browser.togglePackage(browser.items[1]);
        mouseClick(findChild(browser, "upgradeAllButton"));
        compare(JSON.stringify(JSON.parse(fake.lastChecked)),
            JSON.stringify([["flatpak", "io.github.astrovm.AdventureMods", "x86_64", "astrovm", "system", reference]]));
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
    function test_settings_contains_version_and_github_link() {
        compare(browser.repositoryUrl.toString(), "https://github.com/astrovm/PkgDeck");
        browser.openView("Settings");
        const about = findChild(browser, "aboutText");
        verify(about.visible);
        verify(about.text.indexOf("9.9.9-test") >= 0);
        const link = findChild(browser, "repositoryLink");
        verify(link.visible);
        compare(link.text, "GitHub");
        const icon = findChild(browser, "repositoryLinkIcon");
        tryCompare(icon, "status", Image.Ready);
        verify(icon.visible);
        let iconPosition = icon.mapToItem(link.contentItem, 0, 0);
        verify(iconPosition.x >= 0);
        verify(iconPosition.x + icon.width <= link.contentItem.width);
        link.forceActiveFocus();
        verify(link.activeFocus);
        browser.width = 380;
        browser.height = 500;
        wait(30);
        verify(link.visible);
        verify(link.width <= findChild(browser, "settingsScroll").availableWidth);
        iconPosition = icon.mapToItem(link.contentItem, 0, 0);
        verify(iconPosition.x >= 0);
        verify(iconPosition.x + icon.width <= link.contentItem.width);
    }
    function test_search_button_fits_at_normal_and_compact_widths() {
        browser.openView("Search");
        const search = findChild(browser, "searchField");
        const button = findChild(browser, "searchButton");
        const label = findChild(browser, "searchButtonLabel");
        search.text = "Firefox";
        compare(browser.queryDirty, true);
        compare(browser.emptyStateMessage(), "");
        for (const width of [1100, 380, 360]) {
            browser.width = width;
            waitForRendering(browser.contentItem);
            verify(button.width >= button.contentItem.implicitWidth + button.leftPadding + button.rightPadding - 1);
            compare(button.height, search.height);
            const labelPosition = label.mapToItem(button, 0, 0);
            verify(labelPosition.x > 0);
            verify(labelPosition.x + label.width < button.width);
        }
        mouseClick(button);
        compare(fake.lastQuery, "Firefox");
        compare(browser.queryDirty, false);
    }
    function test_dynamic_buttons_fit_at_wide_and_compact_widths() {
        browser.openView("Updates");
        browser.viewSourceFilters = ({Updates: ["claude"]});
        fake.rows = JSON.stringify([
            {kind: "package", name: "synthetic-editor", source: "apt", architecture: "all", installed: "1", candidate: "2", update: "available", scope: "system"},
            {kind: "package", name: "synthetic-player", source: "flatpak", architecture: "x86_64", installed: "1", candidate: "2", update: "available", scope: "user"}
        ]);
        const filter = findChild(browser, "sourceFilter");
        const compactFilter = findChild(browser, "compactSourceFilter");
        const list = findChild(browser, "packageResults");
        const fits = (button) => {
            const contents = button.contentItem.children[0];
            const at = contents.mapToItem(button.contentItem, 0, 0);
            verify(at.x >= -1, button.objectName + " starts outside its content area");
            verify(at.x + contents.width <= button.contentItem.width + 1,
                button.objectName + " overflows its content area");
            const label = contents.children.find((item) => item.text !== undefined);
            const labelAt = label.mapToItem(button.contentItem, 0, 0);
            verify(labelAt.x + label.width <= button.contentItem.width + 1,
                button.objectName + " label overflows its content area");
        };
        browser.width = 1100;
        waitForRendering(browser.contentItem);
        verify(filter.width >= Math.min(240, filter.implicitWidth) - 1);
        fits(filter);
        browser.viewSourceFilters = ({});
        tryVerify(() => list.itemAtIndex(0) !== null);
        for (let index = 0; index < 2; index++) {
            const action = findChild(list.itemAtIndex(index), "rowPackageAction");
            compare(action.text, "");
            compare(action.width, 38);
            fits(action);
        }
        browser.togglePackage(JSON.parse(fake.rows)[0]);
        const update = findChild(browser, "upgradeAllButton");
        compare(update.text, "Update selected (1)");
        for (const name of ["upgradeAllButton", "selectNoneButton", "selectAllButton"]) {
            const button = findChild(browser, name);
            verify(button.width >= button.contentItem.implicitWidth + button.leftPadding + button.rightPadding - 1,
                name + " is too narrow");
        }
        browser.width = 360;
        browser.viewSourceFilters = ({Updates: ["claude"]});
        waitForRendering(browser.contentItem);
        fits(compactFilter);
    }
    function test_narrow_windows_use_layout_that_keeps_header_actions_inside() {
        browser.openView("Updates");
        const add = findChild(browser, "addPackageButton");
        const activity = findChild(browser, "activityIndicator");
        const sources = findChild(browser, "sourceFilter");
        const compactActivity = findChild(browser, "compactActivityIndicator");
        const compactSources = findChild(browser, "compactSourceFilter");
        const navigation = findChild(browser, "navigationView");
        for (const width of [360, 760, 800, 820, 850, 920, 960]) {
            browser.width = width;
            waitForRendering(browser.contentItem);
            compare(browser.compact, width < 960);
            compare(browser.navigationCollapsed, width < 820);
            compare(navigation.visible, width < 820);
            verify(!add.visible);
            const buttons = width < 820 ? [compactActivity, compactSources] : [activity, sources];
            for (const button of buttons) {
                verify(button.visible);
                verify(button.text.length > 0);
                const left = button.mapToItem(browser.contentItem, 0, 0).x;
                const right = button.mapToItem(browser.contentItem, button.width, 0).x;
                verify(left >= 0 && right <= width, button.objectName + " clips at " + width);
            }
        }
    }
    function test_short_result_lists_fit_their_rows() {
        browser.openView("Installed");
        fake.rows = JSON.stringify([
            {kind: "package", name: "synthetic-one", source: "apt", installed: "1", candidate: "1", scope: "system", summary: "First package"},
            {kind: "package", name: "synthetic-two", source: "apt", installed: "2", candidate: "2", scope: "system", summary: "Second package"}
        ]);
        const box = findChild(browser, "resultsBox");
        const list = findChild(browser, "packageResults");
        for (const width of [1100, 380]) {
            browser.width = width;
            waitForRendering(browser.contentItem);
            compare(list.count, 2);
            verify(box.height < 300, "Short list stretches at " + width);
            verify(list.itemAtIndex(1) !== null);
        }
        browser.width = 1100;
        fake.rows = JSON.stringify(Array.from({length: 8}, (_, index) => ({
            kind: "package", name: "synthetic-" + index, source: "apt", installed: "1", candidate: "1", scope: "system", summary: "Synthetic package"
        })));
        waitForRendering(browser.contentItem);
        browser.choose(0);
        waitForRendering(browser.contentItem);
        const actions = findChild(browser, "updatesActions");
        const bottom = actions.mapToItem(browser.contentItem, 0, actions.height).y;
        verify(bottom <= browser.height, "Details push actions outside the window");
    }
    function test_focus_results_shortcut_only_works_on_visible_results() {
        const list = findChild(browser, "packageResults");
        browser.openView("Settings");
        keyClick(Qt.Key_L, Qt.ControlModifier);
        verify(!list.activeFocus);
        browser.openView("Installed");
        fake.rows = JSON.stringify([{kind: "package", name: "synthetic-one", source: "apt", installed: "1", candidate: "1", scope: "system"}]);
        waitForRendering(browser.contentItem);
        keyClick(Qt.Key_L, Qt.ControlModifier);
        tryCompare(list, "activeFocus", true);
    }
    function test_file_or_link_entry_is_contextual_and_validates_links() {
        const add = findChild(browser, "addPackageButton");
        const activity = findChild(browser, "activityIndicator");
        const filter = findChild(browser, "sourceFilter");
        const compactActivity = findChild(browser, "compactActivityIndicator");
        const compactFilter = findChild(browser, "compactSourceFilter");
        browser.openView("Search");
        verify(add.visible);
        for (const width of [1100, 360]) {
            browser.width = width;
            waitForRendering(browser.contentItem);
            verify((width < 820 ? compactActivity : activity).visible);
            verify((width < 820 ? compactFilter : filter).visible);
            verify(add.visible);
            const right = add.mapToItem(browser.contentItem, add.width, 0).x;
            verify(right <= browser.width);
        }
        clickDelegate(add);
        const dialog = findChild(browser, "addPackageDialog");
        tryCompare(dialog, "opened", true);
        const link = findChild(browser, "packageLink");
        const preview = findChild(browser, "previewPackageLink");
        link.text = "https://example.invalid/app.flatpakref";
        verify(preview.enabled);
        link.text = "http://example.invalid/app.rpm";
        verify(!preview.enabled);
        link.text = "https://example.invalid/app.rpm";
        verify(preview.enabled);
        clickDelegate(preview);
        compare(fake.lastOpenedInput, "https://example.invalid/app.rpm");
        clickDelegate(add);
        tryCompare(dialog, "opened", true);
        link.text = "flatpak+https://example.invalid/app.flatpakref";
        verify(preview.enabled);
        clickDelegate(preview);
        compare(fake.lastOpenedInput, "flatpak+https://example.invalid/app.flatpakref");
        browser.openView("Installed");
        verify(!add.visible);
        browser.openView("Settings");
        verify(!add.visible);
        verify(!filter.visible);
        verify(compactActivity.visible);
    }
    function test_source_filter_button_toggles_its_popup() {
        const popup = findChild(browser, "sourcePopup");
        for (const width of [1100, 380]) {
            browser.width = width;
            browser.openView("Search");
            waitForRendering(browser.contentItem);
            const button = findChild(browser, width < 820 ? "compactSourceFilter" : "sourceFilter");
            const checks = fake.sourceChecks;
            clickDelegate(button);
            tryCompare(popup, "visible", true);
            compare(fake.sourceChecks, checks + 1);
            clickDelegate(button);
            tryCompare(popup, "visible", false);
            compare(fake.sourceChecks, checks + 1);
            clickDelegate(button);
            tryCompare(popup, "visible", true);
            popup.close();
            tryCompare(popup, "visible", false);
        }
    }
    function test_returning_to_search_reloads_visible_query() {
        browser.openView("Search");
        const search = findChild(browser, "searchField");
        search.text = "fixture";
        mouseClick(findChild(browser, "searchButton"));
        fake.rows = JSON.stringify([{kind: "package", name: "fixture", source: "homebrew", architecture: "all", installed: null, candidate: "1.0", scope: "user", summary: "Synthetic package"}]);
        compare(browser.viewItems.length, 1);
        browser.openView("Installed");
        fake.rows = "[]";
        const before = fake.loadCount;
        browser.openView("Search");
        compare(fake.loadCount, before + 1);
        compare(fake.lastView, "Search");
        compare(fake.lastQuery, "fixture");
        fake.rows = JSON.stringify([{kind: "package", name: "fixture", source: "homebrew", architecture: "all", installed: null, candidate: "1.0", scope: "user", summary: "Synthetic package"}]);
        compare(browser.viewItems.length, 1);
    }
    function test_returning_to_empty_search_hides_previous_section_rows() {
        browser.openView("Sources");
        fake.rows = JSON.stringify([{kind: "source", name: "APT", source: "apt", summary: "Available", available: true}]);
        fake.report_state = JSON.stringify({phase: "partial", failures: [{source: "apt", kind: "failed", detail: "Synthetic source failure"}]});
        compare(browser.viewItems.length, 1);
        compare(browser.readFailures.length, 1);

        browser.openView("Search");
        compare(findChild(browser, "searchField").text, "");
        compare(fake.lastView, "Search");
        compare(fake.lastQuery, "");
        compare(browser.items.length, 0);
        compare(browser.viewItems.length, 0);
        compare(browser.readFailures.length, 0);
        verify(!findChild(browser, "resultsBox").visible);
        fake.busy = true;
        verify(!findChild(browser, "resultsBox").visible);
        fake.busy = false;

        browser.reload(true);
        compare(browser.viewItems.length, 0);
        verify(!findChild(browser, "resultsBox").visible);

        const search = findChild(browser, "searchField");
        search.text = "fixture";
        mouseClick(findChild(browser, "searchButton"));
        fake.rows = JSON.stringify([{kind: "package", name: "fixture", source: "apt", candidate: "1.0", summary: "Synthetic package"}]);
        compare(browser.viewItems.length, 1);
    }
    function test_search_field_stays_put_during_loading_and_results() {
        browser.width = 1100;
        browser.height = 700;
        browser.openView("Settings");
        fake.simulateLoading = true;
        browser.openView("Search");
        waitForRendering(browser.contentItem);
        const field = findChild(browser, "searchField");
        const top = field.mapToItem(browser.contentItem, 0, 0).y;
        field.text = "synthetic";
        mouseClick(findChild(browser, "searchButton"));
        waitForRendering(browser.contentItem);
        compare(field.mapToItem(browser.contentItem, 0, 0).y, top);
        fake.rows = JSON.stringify(Array.from({length: 23}, (_, index) => ({
            kind: "package", name: "synthetic-" + index, source: "apt", candidate: "1", scope: "system"
        })));
        fake.busy = false;
        waitForRendering(browser.contentItem);
        compare(field.mapToItem(browser.contentItem, 0, 0).y, top);
    }
    function test_result_status_stays_short_during_native_writes() {
        browser.openView("Updates");
        fake.status = "Update all packages from apt: Running apt-get; cancellation waits for the native transaction to finish.";
        fake.busy = true;
        fake.writing = true;
        compare(browser.resultsHeading(), "Applying changes…");
        compare(findChild(browser, "resultsHeading").text, "Applying changes…");
    }
    function test_opening_input_has_visible_feedback_until_preview_or_completion() {
        fake.simulateOpening = true;
        browser.openExternalInput("file:///tmp/synthetic.deb");
        compare(fake.lastOpenedInput, "file:///tmp/synthetic.deb");
        const notice = findChild(browser, "openingNotice");
        verify(notice.visible);
        fake.confirmation_data = JSON.stringify({action: "Install", summary: "Install synthetic-tool"});
        fake.confirmation = "Install synthetic-tool";
        tryCompare(notice, "visible", false);
        findChild(browser, "confirmationDialog").reject();
        fake.simulateOpening = true;
        browser.openExternalInput("file:///tmp/invalid.deb");
        verify(notice.visible);
        fake.busy = false;
        tryCompare(notice, "visible", false);
    }
    function test_activity_uses_short_readable_actions() {
        browser.openView("Activity");
        const activity = findChild(browser, "activityPane");
        compare(activity.target({upgrade_all: {backend: "apt"}}), "Update all · apt");
        compare(activity.target({install: {backend: "apt", name: "synthetic-tool", scope: "system"}}), "Install synthetic-tool · apt · System");
        compare(activity.result({state: "running", outcomes: []}), "In progress");
    }
    function test_action_progress_shows_batch_steps_and_single_transfer() {
        browser.openView("Updates");
        fake.writing = true;
        fake.progress = JSON.stringify({activity_id: 42, label: "Update synthetic-tool from apt", done: 2, total: 3});
        const strip = findChild(browser, "operationProgress");
        const bar = findChild(strip, "actionProgressBar");
        verify(strip.visible);
        compare(strip.label, "Update synthetic-tool from apt");
        compare(bar.indeterminate, false);
        compare(bar.value, 2);
        compare(bar.to, 3);
        browser.openView("Activity");
        verify(strip.visible);
        fake.activity = JSON.stringify([{id: 42, state: "running", started_at: 1,
            operations: [{upgrade: {backend: "apt", name: "synthetic-tool", scope: "system"}}], outcomes: []}]);
        verify(!strip.visible);
        const list = findChild(browser, "activityList");
        tryVerify(() => list.itemAtIndex(0) !== null);
        const activityProgress = findChild(list.itemAtIndex(0), "activityProgress");
        verify(activityProgress.visible);
        compare(findChild(activityProgress, "actionProgressBar").value, 2);
        fake.progress = JSON.stringify({activity_id: 42, label: "Install synthetic-tool from apt", done: 0, total: 1});
        compare(findChild(activityProgress, "actionProgressBar").indeterminate, true);
        fake.progress = JSON.stringify({activity_id: 42, label: "Install synthetic-tool from apt", done: 0, total: 1,
            transferred: 50, transfer_total: 100});
        compare(findChild(activityProgress, "actionProgressBar").indeterminate, false);
        compare(findChild(activityProgress, "actionProgressBar").value, 50);
        fake.writing = false;
        verify(!strip.visible);
    }
    function test_confirmation_shows_summary_before_optional_details() {
        browser.openView("Search");
        fake.confirmation_data = JSON.stringify({action: "Install", summary: "Install synthetic-tool\nSource: apt\nScope: System", details: "Additional dependency: synthetic-library"});
        fake.confirmation = "Install synthetic-tool";
        const dialog = findChild(browser, "confirmationDialog");
        tryCompare(dialog, "opened", true);
        compare(findChild(browser, "confirmationSummary").text, "Install synthetic-tool");
        compare(findChild(browser, "confirmationSummaryMeta").text, "Source: apt\nScope: System");
        const summary = findChild(browser, "confirmationSummary");
        verify(summary.mapToItem(dialog.contentItem, 0, 0).x >= 20);
        const details = findChild(browser, "confirmationDetails");
        verify(!details.visible);
        clickDelegate(findChild(browser, "confirmationDetailsButton"));
        verify(details.visible);
        verify(details.text.indexOf("synthetic-library") >= 0);
        verify(details.mapToItem(dialog.contentItem, 0, 0).x >= 20);
        dialog.reject();
    }
    function test_compact_confirmation_keeps_both_buttons_readable() {
        browser.width = 360;
        browser.height = 520;
        fake.confirmation_data = JSON.stringify({action: "Update 1 package", summary: "Update 1 package", details: "Synthetic update details"});
        fake.confirmation = "Update synthetic package";
        const dialog = findChild(browser, "confirmationDialog");
        tryCompare(dialog, "opened", true);
        waitForRendering(browser.contentItem);
        const apply = findChild(dialog, "confirmationApply");
        const cancel = findChild(dialog, "confirmationCancel");
        compare(apply.text, "Update");
        compare(cancel.text, "Cancel");
        verify(cancel.width >= cancel.contentItem.implicitWidth + cancel.leftPadding + cancel.rightPadding - 1);
        verify(dialog.height < 260);
        dialog.reject();
    }
    function test_empty_results_are_compact_and_search_starts_clear() {
        browser.openView("Search");
        const box = findChild(browser, "resultsBox");
        waitForRendering(browser.contentItem);
        verify(!box.visible);
        verify(!findChild(browser, "reloadButton").visible);
        browser.openView("Updates");
        fake.report_state = JSON.stringify({phase: "complete", failures: []});
        waitForRendering(browser.contentItem);
        verify(box.visible);
        verify(box.height <= 160);
        verify(!findChild(browser, "columnHeader0").visible);
        compare(findChild(browser, "emptyState").text, "You're up to date");
    }
    function test_new_search_does_not_show_previous_query_results() {
        browser.openView("Search");
        const search = findChild(browser, "searchField");
        search.text = "vlc";
        browser.reload();
        fake.rows = JSON.stringify([{kind: "package", name: "vlc", source: "apt", installed: null, candidate: "1", scope: "system", summary: "Synthetic player"}]);
        waitForRendering(browser.contentItem);
        compare(browser.viewItems.length, 1);

        search.text = "firefox";
        fake.simulateLoading = true;
        search.forceActiveFocus();
        keyClick(Qt.Key_Return);
        compare(browser.retainingResults, false);
        compare(browser.viewItems.length, 0);
        compare(findChild(browser, "resultsHeading").text, "Searching…");
        verify(!findChild(browser, "emptyState").visible);
        compare(findChild(browser, "emptyState").text, "");
    }
    function test_compact_installed_filter_has_its_own_row() {
        browser.width = 360;
        browser.openView("Installed");
        waitForRendering(browser.contentItem);
        const field = findChild(browser, "installedFilterField");
        verify(field.width >= 300);
    }
    function test_settings_shows_shortcuts_and_scrolls() {
        browser.openView("Settings");
        const about = findChild(browser, "aboutText");
        verify(about !== null);
        verify(about.text.indexOf("9.9.9-test") >= 0);
        const shortcuts = findChild(browser, "aboutShortcuts");
        verify(shortcuts !== null);
        compare(shortcuts.count, 15);
        verify(findChild(browser, "shortcutsToggle") === null);
        waitForRendering(browser.contentItem);
        compare(shortcuts.itemAt(0).children[1].text, "Ctrl+1");
        compare(shortcuts.itemAt(11).children[1].text, "Ctrl+Shift+U");
        for (const name of ["animationsSetting", "backgroundModeSetting"]) {
            const setting = findChild(browser, name);
            compare(setting.contentItem.color.toString(), browser.ink.toString());
        }
        waitForRendering(browser.contentItem);
        const appearance = findChild(browser, "appearanceSetting");
        const at = appearance.mapToItem(browser.contentItem, 0, 0);
        verify(at.y < browser.height / 3);
        browser.height = 1300;
        waitForRendering(browser.contentItem);
        const tall = appearance.mapToItem(browser.contentItem, 0, 0);
        verify(tall.y < browser.height / 3);
        browser.width = 360;
        browser.height = 400;
        waitForRendering(browser.contentItem);
        const scroll = findChild(browser, "settingsScroll");
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
        verify(findChild(browser, "aboutText").visible);
        browser.openView("Sources");
        compare(findChild(browser, "columnHeader0").text, "SOURCE");
        compare(findChild(browser, "columnHeader1").text, "STATUS");
        verify(!findChild(browser, "columnHeader2").visible);
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
        browser.openView("Search");
        compare(findChild(browser, "columnHeader0").text, "NAME / SOURCE");
        compare(findChild(browser, "columnHeader1").text, "VERSION");
        compare(findChild(browser, "columnHeader2").text, "SUMMARY");
        compare(browser.items.length, 0);
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
        // Submitting a search resets to best-match order without discarding
        // unrelated cached views.
        search.forceActiveFocus();
        keyClick(Qt.Key_Return);
        compare(browser.sortColumn, "");
        compare(browser.viewItems[0].name, "fire");
        compare(fake.lastForce, false);
        compare(fake.lastView, "Search");
    }
    function test_activity_navigation_stays_available_during_write() {
        populate();
        fake.activity = JSON.stringify([{id: 1, frontend: "gui", operations: [{install: {backend: "apt", name: "synthetic-tool", architecture: "all", scope: "system"}}], started_at: 1000, state: "queued", outcomes: []}]);
        fake.writing = true;
        fake.busy = true;
        browser.openView("Installed");
        compare(browser.currentView, "Installed");
        verify(findChild(findChild(browser, "packageResults").itemAtIndex(0), "rowPackageAction").enabled);
        verify(findChild(browser, "activityIndicator").visible);
        browser.openView("Activity");
        compare(browser.currentView, "Activity");
        const list = findChild(browser, "activityList");
        tryCompare(list, "count", 1);
        verify(findChild(browser, "cancelQueuedButton").enabled);
        fake.writing = false;
        fake.busy = false;
    }
    function test_activity_has_a_name_in_compact_navigation() {
        browser.width = 380;
        browser.openView("Activity");
        waitForRendering(browser.contentItem);
        const navigation = findChild(browser, "navigationView");
        compare(navigation.currentText, "Activity");
        verify(!findChild(browser, "activityIndicator").visible);
        browser.width = 1100;
        waitForRendering(browser.contentItem);
        verify(!findChild(browser, "activityIndicator").visible);
    }
    function test_background_mode_requires_a_usable_tray_to_hide() {
        browser.openView("Settings");
        browser.startHidden = true;
        browser.backgroundMode = true;
        browser.trayAvailable = false;
        verify(browser.visible);
        verify(!findChild(browser, "autostartSetting").enabled);
        browser.trayAvailable = true;
        verify(!browser.visible);
        browser.backgroundMode = false;
        verify(browser.visible);
        browser.close();
        verify(!browser.visible);
    }
    function test_container_reference_uses_explicit_search_submission() {
        browser.openView("Search");
        const search = findChild(browser, "searchField");
        const requests = fake.loadCount;
        search.text = "registry.example/team/app:tag";
        compare(fake.loadCount, requests);
        search.forceActiveFocus();
        keyClick(Qt.Key_Return);
        compare(fake.lastQuery, "registry.example/team/app:tag");
        compare(fake.lastView, "Search");
        compare(fake.writes, 0);
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
        verify(actionsPosition.y + actions.height <= browser.contentItem.height,
            "actions: y=" + actionsPosition.y + " height=" + actions.height +
            " window=" + browser.contentItem.height + " results=" + findChild(browser, "resultsBox").height +
            " details=" + panel.height);
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
        verify(!findChild(browser, "detailsPanel").visible);
        fake.details = JSON.stringify({package: JSON.parse(fake.rows)[1], description: "Additional details"});
        const close = findChild(browser, "closeDetailsButton");
        compare(close.text, "");
        mouseClick(close);
        verify(browser.selected === null);
        verify(!findChild(browser, "detailsPanel").visible);
        compare(fake.writes, 0);
    }
    function test_details_show_only_information_beyond_the_selected_row() {
        populate();
        browser.choose(0);
        const row = JSON.parse(fake.rows)[0];
        const panel = findChild(browser, "detailsPanel");
        const description = findChild(browser, "packageDetails");
        const metadata = findChild(browser, "packageMetadata");
        fake.details = JSON.stringify({package: row, description: row.summary,
            available_sources: [row], installed_copies: [row]});
        compare(browser.detailText(), "");
        verify(!description.visible);
        verify(!metadata.visible);
        verify(!panel.visible);
        verify(panel.idealHeight < 130);
        fake.details = JSON.stringify({package: row, description: "A longer description from the package source.",
            publisher: "Example publisher", license: "MIT", homepage: "https://example.invalid/app",
            dependencies: ["synthetic-library"]});
        compare(description.text, "A longer description from the package source.");
        verify(description.visible);
        verify(metadata.visible);
        verify(panel.visible);
        verify(metadata.text.indexOf("Publisher: Example publisher") >= 0);
        verify(metadata.text.indexOf("License: MIT") >= 0);
        verify(metadata.text.indexOf("Homepage: https://example.invalid/app") >= 0);
        verify(metadata.text.indexOf("Dependencies: synthetic-library") >= 0);
        verify(metadata.text.indexOf("Identity:") < 0);
        verify(metadata.text.indexOf("Scope:") < 0);
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
        compare(dialog.background.color.toString(), browser.surface.toString());
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
    function test_repository_form_validates_before_submitting() {
        const dialog = findChild(browser, "addRepositoryDialog");
        dialog.open();
        tryCompare(dialog, "visible", true);
        const ok = dialog.standardButton(Controls.Dialog.Ok);
        verify(ok !== null);
        const name = findChild(browser, "repositoryName");
        const url = findChild(browser, "repositoryUrl");
        verify(!ok.enabled);
        name.text = "-invalid";
        url.text = "http://example.invalid/repo.flatpakrepo";
        verify(findChild(browser, "repositoryNameError").visible);
        verify(findChild(browser, "repositoryUrlError").visible);
        clickDelegate(ok);
        verify(dialog.visible);
        compare(fake.lastRepositoryChange, "");

        name.text = "sample_repo";
        url.text = "https://example.invalid/repo.flatpakrepo";
        tryVerify(() => ok.enabled);
        verify(!findChild(browser, "repositoryNameError").visible);
        verify(!findChild(browser, "repositoryUrlError").visible);
        clickDelegate(ok);
        tryCompare(dialog, "visible", false);
        const request = JSON.parse(fake.lastRepositoryChange);
        compare(request.action, "add");
        compare(request.name, "sample_repo");
        compare(request.url, "https://example.invalid/repo.flatpakrepo");
    }
    function test_narrow_repositories_show_identity_before_actions() {
        browser.width = 360;
        browser.height = 500;
        browser.openView("Sources");
        const title = "A synthetic repository with a long name";
        const url = "https://example.invalid/long/path/to/a/synthetic-repository.flatpakrepo";
        fake.repositories = JSON.stringify({repositories: [{backend: "flatpak", name: "synthetic-repo", title: title, url: url, scope: "user", enabled: true, priority: 1}], errors: []});
        const dialog = findChild(browser, "repositoriesDialog");
        dialog.open();
        tryCompare(dialog, "visible", true);
        const list = findChild(browser, "repositoryList");
        tryVerify(() => list.itemAtIndex(0) !== null);
        waitForRendering(browser.contentItem);
        const row = list.itemAtIndex(0);
        const titleLabel = findChild(row, "repositoryTitle");
        const urlLabel = findChild(row, "repositoryUrlLabel");
        const remove = findChild(row, "removeRepositoryButton");
        compare(titleLabel.text, title);
        compare(urlLabel.text, url);
        verify(row.height > 76);
        verify(urlLabel.height > urlLabel.font.pointSize);
        const urlBottom = urlLabel.mapToItem(row, 0, urlLabel.height).y;
        const actionTop = remove.mapToItem(row, 0, 0).y;
        verify(urlBottom <= actionTop);
        dialog.close();
    }
    function test_repositories_do_not_repeat_url_in_title() {
        browser.openView("Sources");
        const url = "https://example.invalid/packages";
        fake.repositories = JSON.stringify({repositories: [{backend: "apt", name: "synthetic", title: url + " stable", url: url, scope: "system", enabled: true}], errors: []});
        const dialog = findChild(browser, "repositoriesDialog");
        dialog.open();
        tryCompare(dialog, "visible", true);
        const list = findChild(browser, "repositoryList");
        tryVerify(() => list.itemAtIndex(0) !== null);
        verify(!findChild(list.itemAtIndex(0), "repositoryUrlLabel").visible);
        verify(dialog.height < 400);
        dialog.close();
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
        compare(pull.text, "");
        compare(pull.width, 38);
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
    function test_source_picker_stays_inside_window() {
        browser.openView("Updates");
        const popup = findChild(browser, "sourcePopup");
        const apply = findChild(browser, "applySourceFilter");
        for (const width of [1100, 420, 360]) {
            browser.width = width;
            popup.open();
            tryCompare(popup, "visible", true);
            waitForRendering(browser.contentItem);
            for (const item of [popup.contentItem, apply]) {
                const at = item.mapToItem(browser.contentItem, 0, 0);
                verify(at.x >= 0);
                verify(at.x + item.width <= browser.width);
            }
            popup.close();
            tryCompare(popup, "visible", false);
        }
    }
    function test_source_picker_shrinks_to_few_sources() {
        browser.width = 360;
        browser.height = 520;
        browser.openView("Search");
        fake.source_catalog = JSON.stringify(["apt", "homebrew", "flatpak"].map(source => ({source, summary: "Available", availability_kind: "available", capabilities: ["search"]})));
        const popup = findChild(browser, "sourcePopup");
        popup.open();
        tryCompare(popup, "visible", true);
        waitForRendering(browser.contentItem);
        verify(popup.height < 360);
        popup.close();
    }
    function test_source_picker_focus_and_page_visibility() {
        browser.openView("Updates");
        const filter = findChild(browser, "sourceFilter");
        clickDelegate(filter);
        const popup = findChild(browser, "sourcePopup");
        tryCompare(popup, "visible", true);
        tryVerify(() => findChild(browser, "sourcePickerSearch").activeFocus);
        verify(findChild(browser, "unavailableSourceToggle").visible);
        popup.close();
        tryCompare(popup, "visible", false);

        browser.openView("Settings");
        verify(!filter.visible);
        browser.openView("Sources");
        verify(!filter.visible);
    }
    function test_compact_updates_keep_names_and_actions_readable() {
        browser.width = 360;
        browser.openView("Updates");
        fake.upgradable = true;
        const rows = [
            {kind: "package", name: "synthetic-editor", display_name: "Synthetic Editor", source: "apt", architecture: "all", installed: "1.0", candidate: "2.0", update: "available", scope: "system", summary: "Edit synthetic documents"},
            {kind: "package", name: "synthetic-player", display_name: "Synthetic Player", source: "flatpak", architecture: "x86_64", installed: "3.1", candidate: "3.2", update: "available", scope: "user", summary: "Play synthetic music"}
        ];
        fake.rows = JSON.stringify(rows);
        const list = findChild(browser, "packageResults");
        tryVerify(() => list.itemAtIndex(0) !== null);
        waitForRendering(browser.contentItem);
        const row = list.itemAtIndex(0);
        verify(findChild(row, "packageName").width >= 140);
        verify(findChild(row, "compactVersion").visible);
        verify(!findChild(row, "wideVersion").visible);
        const update = findChild(browser, "upgradeAllButton");
        const selectNone = findChild(browser, "selectNoneButton");
        const reload = findChild(browser, "reloadButton");
        compare(reload.text, "");
        const updateY = update.mapToItem(browser.contentItem, 0, 0).y;
        compare(selectNone.mapToItem(browser.contentItem, 0, 0).y, updateY);
        compare(reload.mapToItem(browser.contentItem, 0, 0).y, updateY);
        browser.togglePackage(rows[0]);
        compare(update.text, "Update");
        compare(reload.mapToItem(browser.contentItem, 0, 0).y, update.mapToItem(browser.contentItem, 0, 0).y);
    }
    function test_results_leave_room_for_the_scrollbar() {
        browser.width = 1100;
        browser.height = 400;
        browser.openView("Installed");
        fake.rows = JSON.stringify(Array.from({length: 20}, (_, index) => ({
            kind: "package", name: "synthetic-" + index, source: "apt",
            installed: "1", candidate: "1", scope: "system"
        })));
        const list = findChild(browser, "packageResults");
        tryVerify(() => list.itemAtIndex(0) !== null);
        waitForRendering(browser.contentItem);
        const row = list.itemAtIndex(0);
        const action = findChild(row, "rowPackageAction");
        compare(list.width - row.width, 16);
        verify(action.mapToItem(list, action.width, 0).x <= row.width);
    }
    function test_header_source_checklist_and_installed_filter() {
        browser.openView("Installed");
        const filter = findChild(browser, "sourceFilter");
        verify(filter !== null);
        compare(filter.text, "Filter sources");
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
        compare(filter.text, "Filter sources");
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
        verify(!findChild(browser, "resultsBox").visible);
        mouseClick(findChild(browser, "sourceFailureDetails"));
        const dialog = findChild(browser, "sourceFailuresDialog");
        tryCompare(dialog, "visible", true);
        verify(browser.failureHelp("failed").indexOf("Retry this source") >= 0);
        verify(browser.failureHelp("failed").indexOf("Source failed") < 0);
        verify(dialog.height < 320);
        compare(browser.copyableDiagnostics(), "View: Search\nState: unknown\nnpm: failed — Source failed");
        browser.retryFailedSource("npm");
        compare(fake.lastRetry, "npm");
    }
    function test_sidebar_shows_app_logo() {
        const logo = findChild(browser, "appLogo");
        verify(logo !== null);
        verify(logo.source.toString().indexOf("logo.svg") >= 0);
        tryCompare(logo, "status", Image.Ready);
    }
    function test_signature_fits_sidebar_and_compact_settings() {
        const footer = findChild(browser, "signatureFooter");
        verify(footer.visible);
        verify(footer.width <= 164);
        verify(footer.y > browser.height / 2);
        verify(findChild(browser, "sidebarRepositoryLink") === null);
        verify(findChild(browser, "signaturePrefix").width >= findChild(browser, "signaturePrefix").implicitWidth);
        verify(findChild(browser, "signatureAuthor").width >= findChild(browser, "signatureAuthor").implicitWidth);
        browser.width = 380;
        browser.openView("Settings");
        wait(30);
        verify(!footer.visible);
        verify(findChild(browser, "compactSignature").visible);
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
        clickDelegate(findChild(browser, "sourceFilter"));
        tryCompare(popup, "visible", true);
        compare(fake.sourceChecks, 1);
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
        compare(browser.emptyStateMessage(), "");
        fake.report_state = JSON.stringify({phase: "complete", failures: []});
        compare(browser.emptyStateMessage(), "You're up to date");
        fake.report_state = JSON.stringify({phase: "partial", failures: [{source: "npm", kind: "locked", detail: "Synthetic package lock"}]});
        verify(browser.emptyStateMessage().indexOf("completed") >= 0);
        verify(browser.emptyStateMessage() !== "You're up to date");
        compare(browser.copyableDiagnostics(), "View: Updates\nState: partial\nnpm: locked — Synthetic package lock");
        fake.report_state = JSON.stringify({phase: "failed", failures: [{source: "apt", kind: "authorization"}]});
        compare(browser.emptyStateMessage(), "Could not check these sources.");
        fake.report_state = JSON.stringify({phase: "cached", failures: []});
        compare(browser.emptyStateMessage(), "No updates in the last check.");
        fake.report_state = JSON.stringify({phase: "complete", failures: []});
        fake.rows = JSON.stringify([{kind: "package", name: "anonymous", source: "homebrew", installed: "1", candidate: "2", update: "available"}]);
        browser.viewSourceFilters = ({Updates: ["apt"]});
        compare(browser.viewItems.length, 0);
        compare(browser.emptyStateMessage(), "No results from selected sources.");
        compare(browser.sourceSummary(), "APT");
    }
    function test_sources_can_reenable_a_disabled_manager() {
        browser.openView("Sources");
        fake.rows = JSON.stringify([
            {kind: "source", name: "apt", source: "apt", summary: "Available", available: true, capabilities: ["search"]},
            {kind: "source", name: "npm", source: "npm", summary: "Available", available: true, capabilities: ["search"]}
        ]);
        const list = findChild(browser, "packageResults");
        tryVerify(() => list.itemAtIndex(1) !== null);
        const npmCheck = findChild(list.itemAtIndex(1), "managerEnabled");
        verify(npmCheck.checked);
        clickDelegate(npmCheck);
        verify(browser.sourceSelection.split(",").indexOf("npm") < 0);
        compare(fake.lastSource, ""); // The catalog still loads every manager.
        compare(browser.viewItems.length, 2);
        verify(!npmCheck.checked);
        clickDelegate(npmCheck);
        verify(npmCheck.checked);
        compare(browser.sourceSelection, "");
        browser.width = 360;
        waitForRendering(browser.contentItem);
        const right = npmCheck.mapToItem(browser.contentItem, npmCheck.width, 0).x;
        verify(right <= browser.width);
        browser.openView("Installed");
        const popup = findChild(browser, "sourcePopup");
        popup.open();
        tryCompare(popup, "visible", true);
        browser.toggleDraftSource("apt");
        browser.applySourceDraft();
        compare(browser.sourceSelection, "");
        verify(browser.viewSourceFilters["Installed"] !== undefined);
    }
    function test_compact_source_row_uses_one_name_and_labeled_toggle() {
        browser.width = 360;
        browser.openView("Sources");
        fake.rows = JSON.stringify([{kind: "source", name: "apt", source: "apt", summary: "Available", available: true, capabilities: ["search", "installed", "upgrade", "clean"]}]);
        const list = findChild(browser, "packageResults");
        tryVerify(() => list.itemAtIndex(0) !== null);
        const row = list.itemAtIndex(0);
        compare(findChild(row, "packageName").text, "APT");
        verify(!findChild(row, "packageSourceLine").visible);
        compare(findChild(row, "managerEnabled").text, "Enabled");
    }
    function test_sources_hide_unavailable_until_requested() {
        browser.openView("Sources");
        fake.rows = JSON.stringify([
            {kind: "source", name: "apt", source: "apt", summary: "Available", available: true, capabilities: ["search"]},
            {kind: "source", name: "npm", source: "npm", summary: "Unavailable", available: false, capabilities: []}
        ]);
        const toggle = findChild(browser, "unavailableSourcesButton");
        compare(browser.viewItems.length, 1);
        verify(toggle.visible);
        compare(toggle.text, "Show unavailable (1)");
        browser.width = 360;
        browser.height = 520;
        waitForRendering(browser.contentItem);
        verify(toggle.mapToItem(browser.contentItem, toggle.width, 0).x <= browser.width);
        verify(findChild(browser, "resultsBox").height < 220);
        mouseClick(toggle);
        compare(toggle.text, "Hide unavailable");
        compare(browser.viewItems.length, 2);
        mouseClick(toggle);
        compare(browser.viewItems.length, 1);
    }
    function test_sources_only_show_availability_when_it_differs() {
        browser.openView("Sources");
        fake.rows = JSON.stringify([
            {kind: "source", name: "apt", source: "apt", summary: "Available", available: true, capabilities: ["search"]},
            {kind: "source", name: "docker", source: "docker", summary: "Unavailable", available: false, capabilities: []}
        ]);
        const list = findChild(browser, "packageResults");
        const statusHeader = findChild(browser, "columnHeader1");
        tryCompare(list, "count", 1);
        tryVerify(() => list.itemAtIndex(0) !== null);
        verify(!statusHeader.visible);
        verify(!findChild(list.itemAtIndex(0), "wideVersion").visible);
        verify(findChild(list.itemAtIndex(0), "managerEnabled").visible);
        mouseClick(findChild(browser, "unavailableSourcesButton"));
        tryCompare(list, "count", 2);
        tryVerify(() => list.itemAtIndex(1) !== null);
        verify(statusHeader.visible);
        const unavailable = list.itemAtIndex(1);
        compare(findChild(unavailable, "wideVersion").text, "Unavailable");
        verify(!findChild(unavailable, "managerEnabled").visible);
        browser.width = 380;
        waitForRendering(browser.contentItem);
        verify(findChild(unavailable, "compactVersion").visible);
    }
    function test_repositories_hide_idle_status_but_show_feedback() {
        browser.openView("Sources");
        const dialog = findChild(browser, "repositoriesDialog");
        dialog.open();
        tryCompare(dialog, "opened", true);
        const status = findChild(dialog, "repositoryStatus");
        verify(!status.visible);
        fake.status = "Repository failed";
        verify(status.visible);
        compare(status.text, "Repository failed");
        dialog.close();
    }
    function test_source_picker_uses_discovered_capabilities() {
        fake.source_catalog = JSON.stringify([
            {source: "apt", summary: "Available", availability_kind: "available", capabilities: ["search", "installed", "upgrade"]},
            {source: "npm", summary: "Manager missing", availability_kind: "unavailable", capabilities: ["search", "installed"]}
        ]);
        browser.openView("Updates");
        const popup = findChild(browser, "sourcePopup");
        popup.open();
        tryCompare(popup, "visible", true);
        compare(browser.pickerItems().join(","), "apt");
        popup.showUnavailable = true;
        verify(browser.pickerItems().indexOf("npm") >= 0);
        verify(!browser.sourceCheckAt(browser.sourceIds.indexOf("npm")).enabled);
        popup.close();
    }
    function test_close_requests_cancellation() {
        fake.busy = true;
        browser.close();
        compare(fake.cancels, 1);
    }
}
