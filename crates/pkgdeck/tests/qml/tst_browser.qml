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
        property bool busy: false
        property bool upgradable: false
        property string lastView: ""
        property string lastQuery: ""
        property int selection: -1
        property int writes: 0
        property int cancels: 0
        function load(view, query, source, sudo) {
            lastView = view;
            lastQuery = query;
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
        for (const view of ["Discover", "Installed", "Updates", "Sources", "Settings", "Help / About"]) {
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
        App.DeckIcon { width: 48; height: 48; z: 100 }
    }
    function test_bundled_vectors_render_without_an_icon_font() {
        const icon = createTemporaryObject(vectorIcon, browser.contentItem);
        verify(icon !== null);
        icon.ink = "transparent";
        wait(100);
        const blank = grabImage(icon);
        for (const name of Object.keys(icon.drawings)) {
            icon.name = name;
            icon.ink = "#ffffff";
            wait(100);
            verify(icon.available);
            verify(!grabImage(icon).equals(blank), name + " should paint a bundled vector");
            const light = grabImage(icon);
            icon.ink = "#102030";
            wait(100);
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
    function test_close_requests_cancellation() {
        fake.busy = true;
        browser.close();
        compare(fake.cancels, 1);
    }
}
