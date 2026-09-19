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
        property string rows: "[]"
        property string details: "{}"
        property string status: "Ready"
        property string confirmation: ""
        property string version: "9.9.9-test"
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
            confirmation = action + " synthetic-tool from apt, all, system";
        }
        function proposeChecked(identities) {
            lastChecked = identities;
        }
        function confirm(approved) {
            if (approved)
                writes++;
            confirmation = "";
        }
        function cancel() {
            cancels++;
            busy = false;
        }
        function poll() {
        }
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
        fake.rows = "[]";
        fake.details = "{}";
        fake.status = "Ready";
        fake.confirmation = "";
        fake.busy = false;
        fake.writing = false;
        fake.upgradable = false;
        fake.writes = 0;
        fake.cancels = 0;
        fake.lastChecked = "";
        fake.lastForce = false;
        browser = createTemporaryObject(window, test);
        verify(browser !== null);
        browser.requestActivate();
        // Fresh checklist and column layout per test: QSettings persist
        // across tests in one run.
        browser.sourceSelection = "";
        browser.sortColumn = "";
        browser.sortAscending = true;
        browser.nameWidth = 202;
        browser.versionWidth = 150;
        wait(30);
    }
    function cleanup() {
        browser.close();
    }
    // Popup content reparents to the Overlay, which QTest item clicks
    // reject after the first one ("window not shown"): click by window
    // coordinates mapped through the delegate instead.
    function clickDelegate(delegate) {
        const at = delegate.mapToItem(browser.contentItem, delegate.width / 2, delegate.height / 2);
        mouseClick(browser, at.x, at.y);
    }
    function clickSourceCheck(index) {
        const popup = findChild(browser, "sourcePopup");
        const delegate = browser.sourceCheckAt(index);
        const view = popup.contentItem;
        view.contentY = Math.max(0, Math.min(delegate.y - 8, view.contentHeight - view.height));
        waitForRendering(browser.contentItem);
        clickDelegate(delegate);
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
        verify(findChild(browser, "installButton").enabled);
        waitForRendering(browser.contentItem);
        mouseClick(findChild(browser, "installButton"));
        const dialog = findChild(browser, "confirmationDialog");
        tryCompare(dialog, "opened", true);
        compare(fake.writes, 0);
        dialog.reject();
        compare(fake.writes, 0);
        waitForRendering(browser.contentItem);
        mouseClick(findChild(browser, "installButton"));
        tryCompare(dialog, "opened", true);
        keyClick(Qt.Key_Y, Qt.AltModifier);
        compare(fake.writes, 1);
        list.forceActiveFocus();
        keyClick(Qt.Key_Down);
        compare(fake.selection, 1);
        verify(findChild(browser, "upgradeButton").enabled);
        verify(findChild(browser, "removeButton").enabled);
        verify(!findChild(browser, "installButton").enabled);
        verify(findChild(browser, "packageDetails").text.indexOf("<b>literal metadata</b>") >= 0);
    }
    function test_views_loading_errors_and_resize() {
        for (const view of ["Search", "Installed", "Updates", "Sources", "Settings", "About"]) {
            browser.openView(view);
            compare(browser.currentView, view);
        }
        browser.openView("Installed");
        populate();
        browser.choose(0);
        fake.busy = true;
        verify(!findChild(browser, "installButton").enabled);
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
    function test_same_app_badge_lists_other_sources() {
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
        verify(line.text.indexOf("also in") >= 0);
        verify(line.text.indexOf("Flatpak") >= 0);
        verify(line.text.indexOf("Snap") >= 0);
        compare(browser.sameAppSummary(browser.viewItems[1]), "");
    }
    function test_updates_multiselect_upgrade_selected() {
        browser.openView("Updates");
        populate();
        waitForRendering(browser.contentItem);
        const selectAll = findChild(browser, "selectAllButton");
        verify(selectAll.visible);
        const upgradeSelected = findChild(browser, "upgradeSelectedButton");
        const selectNone = findChild(browser, "selectNoneButton");
        verify(!upgradeSelected.visible);
        mouseClick(selectAll);
        compare(browser.checkedPackages.length, 2);
        waitForRendering(browser.contentItem);
        verify(upgradeSelected.visible);
        verify(selectNone.visible);
        mouseClick(upgradeSelected);
        verify(fake.lastChecked !== "");
        const sent = JSON.parse(fake.lastChecked);
        compare(sent.length, 2);
        // Identities mirror the controller shape: [source, name, arch, remote, scope].
        verify(sent[0].indexOf("synthetic-tool") >= 0);
        verify(sent[0].indexOf("apt") >= 0);
        verify(sent[1].indexOf("homebrew") >= 0);
        mouseClick(selectNone);
        compare(browser.checkedPackages.length, 0);
        verify(!upgradeSelected.visible);
        // Single-row toggle without the header buttons.
        browser.togglePackage(browser.items[1]);
        compare(browser.checkedPackages.length, 1);
        verify(upgradeSelected.visible);
        browser.togglePackage(browser.items[1]);
        compare(browser.checkedPackages.length, 0);
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
        browser.height = 760;
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
        verify(list.activeFocus);
        search.forceActiveFocus();
        search.text = "synthetic";
        fake.rows = JSON.stringify(JSON.parse(fake.rows).reverse());
        wait(30);
        verify(search.activeFocus);
        verify(!list.activeFocus);
        compare(browser.queryDirty, false);
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
        compare(browser.viewItems.length, 6);
        compare(browser.viewItems[0].name, "fire");
        compare(browser.viewItems[0].source, "apt");
        compare(browser.viewItems[1].name, "firefox");
        compare(browser.viewItems[2].name, "x-fire-helper");
        compare(browser.viewItems[3].name, "zzz");
        // A failed source stays above unverified guesses; the guess with an
        // exact name but no version at all sinks to the bottom.
        compare(browser.viewItems[4].kind, "failure");
        compare(browser.viewItems[5].name, "fire");
        compare(browser.viewItems[5].source, "npm");
        // Actions map the visible row back to backend order.
        browser.choose(0);
        compare(fake.selection, 2);
        browser.choose(5);
        compare(fake.selection, 4);
        // An explicit column sort wins over relevance ranking.
        browser.cycleSort("name");
        compare(browser.viewItems[0].name, "bun");
        compare(browser.viewItems[1].name, "fire");
        compare(browser.viewItems[2].name, "fire");
        // Equal names keep engine order, which is not stable: assert the
        // pair as a set instead of a fixed sequence.
        const pair = [browser.viewItems[1].source, browser.viewItems[2].source].sort();
        compare(pair.join(","), "apt,npm");
        compare(browser.viewItems[5].name, "zzz");
        // Submitting a fresh search resets to best-match order and forces
        // a native query instead of serving the cached snapshot.
        search.forceActiveFocus();
        keyClick(Qt.Key_Return);
        compare(browser.sortColumn, "");
        compare(browser.viewItems[0].name, "fire");
        compare(fake.lastForce, true);
        compare(fake.lastView, "Search");
    }
    function test_header_source_checklist_and_installed_filter() {
        browser.openView("Installed");
        const filter = findChild(browser, "sourceFilter");
        verify(filter !== null);
        compare(filter.text, "All available sources");
        const popup = findChild(browser, "sourcePopup");
        verify(popup !== null);
        popup.open();
        tryCompare(popup, "visible", true);
        const npm = browser.sourceIds.indexOf("npm");
        verify(browser.sourceCheckAt(npm).checked);
        clickSourceCheck(npm);
        compare(browser.sourceSelection, "apt,dnf,pacman,zypper,snap,homebrew,appimage,flatpak,cargo,pnpm,bun,pip,pipx,uv,composer,gem");
        compare(filter.text, "16 sources");
        compare(fake.lastSource, "apt,dnf,pacman,zypper,snap,homebrew,appimage,flatpak,cargo,pnpm,bun,pip,pipx,uv,composer,gem");
        compare(fake.lastView, "Installed");
        // Re-checking the last unchecked source returns to all available.
        clickSourceCheck(npm);
        compare(browser.sourceSelection, "");
        compare(filter.text, "All available sources");
        // Unchecking down to one source disables that final checkbox.
        const ids = ["apt", "dnf", "pacman", "zypper", "snap", "homebrew", "appimage", "flatpak", "cargo", "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem"];
        for (let idx = 0; idx < ids.length; idx++) {
            if (ids[idx] === "apt")
                continue;
            clickSourceCheck(browser.sourceIds.indexOf(ids[idx]));
        }
        compare(browser.sourceSelection, "apt");
        compare(filter.text, "APT");
        const last = browser.sourceCheckAt(browser.sourceIds.indexOf("apt"));
        verify(last !== null);
        verify(!last.enabled);
        popup.close();
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
        fake.details = JSON.stringify({
            failure: {backend: "npm", error: "invalid response from npm: npm ls failed: boom"},
            hint: "Check the npm source in the Sources view."
        });
        wait(30);
        const details = findChild(browser, "packageDetails");
        verify(details.text.indexOf("npm ls failed: boom") >= 0);
        verify(details.text.indexOf("Sources view") >= 0);
    }
    function test_sidebar_shows_app_logo() {
        const logo = findChild(browser, "appLogo");
        verify(logo !== null);
        verify(logo.source.toString().indexOf("logo.svg") >= 0);
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
        verify(hint.text.indexOf("source query fails") >= 0);
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
        verify(popup.contentItem.contentHeight > 0);
        const npmCheck = browser.sourceCheckAt(browser.sourceIds.indexOf("npm"));
        verify(npmCheck !== null);
        verify(npmCheck.checked);
        popup.close();
        tryCompare(popup, "visible", false);
    }
    function test_close_requests_cancellation() {
        fake.busy = true;
        browser.close();
        compare(fake.cancels, 1);
    }
}
