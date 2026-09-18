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
        property bool upgradable: false
        property string lastView: ""
        property string lastQuery: ""
        property string lastSource: ""
        property int selection: -1
        property int writes: 0
        property int cancels: 0
        function load(view, query, source, sudo) {
            lastView = view;
            lastQuery = query;
            lastSource = source;
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
        fake.upgradable = false;
        fake.writes = 0;
        fake.cancels = 0;
        browser = createTemporaryObject(window, test);
        verify(browser !== null);
        browser.requestActivate();
        wait(30);
    }
    function cleanup() {
        browser.close();
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
        browser.openView("Updates");
        compare(browser.currentView, "Installed");
        fake.status = "Authorization denied\nNative lock busy\nPartial results: source unavailable";
        compare(findChild(browser, "operationStatus").text, fake.status);
        fake.busy = false;
        verify(findChild(browser, "packageResults").activeFocus);
        browser.width = 380;
        browser.height = 500;
        wait(30);
        verify(findChild(browser, "packageResults").width <= 380);
        verify(findChild(browser, "packageResults").height >= 78);
        verify(findChild(browser, "operationStatus").width > 0);
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
    }
    function test_view_status_and_source_columns() {
        browser.openView("Settings");
        compare(findChild(browser, "operationStatus").text.indexOf("appearance") >= 0, true);
        browser.openView("About");
        compare(findChild(browser, "operationStatus").text, "About PkgDeck.");
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
        keyClick(Qt.Key_Down);
        compare(fake.selection, 0);
        keyClick(Qt.Key_PageDown);
        compare(fake.selection, 1);
        keyClick(Qt.Key_Home);
        compare(fake.selection, 0);
        keyClick(Qt.Key_End);
        compare(fake.selection, 1);
    }
    function test_header_source_filter_and_installed_filter() {
        browser.openView("Installed");
        const filter = findChild(browser, "sourceFilter");
        verify(filter !== null);
        filter.currentIndex = 1;
        filter.activated(1);
        compare(browser.source, "apt");
        compare(fake.lastSource, "apt");
        compare(fake.lastView, "Installed");
        browser.openView("Settings");
        const setting = findChild(browser, "sourceSetting");
        setting.currentIndex = 0;
        setting.activated(0);
        compare(browser.source, "");
        const field = findChild(browser, "installedFilterField");
        verify(field !== null);
        browser.openView("Installed");
        field.forceActiveFocus();
        field.text = "synthetic";
        keyClick(Qt.Key_Return);
        compare(browser.installedFilter, "synthetic");
        compare(fake.lastView, "Installed");
        compare(fake.lastQuery, "synthetic");
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
    function test_upgrade_all_hint_and_state_markers() {
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
    function test_close_requests_cancellation() {
        fake.busy = true;
        browser.close();
        compare(fake.cancels, 1);
    }
}
