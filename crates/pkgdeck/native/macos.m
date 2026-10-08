// The Mac app's menu bar icon, notifications and Dock badge, behind a small
// C interface. AppKit calls back on the main thread; notification responses
// arrive on a framework thread. Both go through `pkgdeck_mac_event`, which
// only queues the event for the window.

#import <AppKit/AppKit.h>
#import <UserNotifications/UserNotifications.h>
#include <stdlib.h>
#include <string.h>

enum { PKGDECK_OPEN = 1, PKGDECK_CHECK = 2, PKGDECK_QUIT = 3, PKGDECK_CLICKED = 4, PKGDECK_ALLOWED = 5, PKGDECK_DENIED = 6 };

static void (*pkgdeck_mac_event)(int) = NULL;
static NSStatusItem *pkgdeck_item = nil;
static BOOL pkgdeck_allowed = NO;

@interface PkgDeckStatusDelegate : NSObject
@end
@implementation PkgDeckStatusDelegate
- (void)open:(id)sender { if (pkgdeck_mac_event) pkgdeck_mac_event(PKGDECK_OPEN); }
- (void)check:(id)sender { if (pkgdeck_mac_event) pkgdeck_mac_event(PKGDECK_CHECK); }
- (void)quit:(id)sender { if (pkgdeck_mac_event) pkgdeck_mac_event(PKGDECK_QUIT); }
@end

@interface PkgDeckNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@end
@implementation PkgDeckNotificationDelegate
// Show banners while PkgDeck is the active app too.
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completionHandler {
    completionHandler(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList);
}
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
    didReceiveNotificationResponse:(UNNotificationResponse *)response
             withCompletionHandler:(void (^)(void))completionHandler {
    if (pkgdeck_mac_event) pkgdeck_mac_event(PKGDECK_CLICKED);
    completionHandler();
}
@end

static PkgDeckStatusDelegate *pkgdeck_status_delegate = nil;
static PkgDeckNotificationDelegate *pkgdeck_notification_delegate = nil;

// The notification center needs a bundle identifier; a bare binary (a
// development build or a test) would crash asking for it.
int pkgdeck_mac_bundled(void) {
    return [[NSBundle mainBundle] bundleIdentifier] != nil;
}

static void pkgdeck_set_allowed(BOOL granted) {
    pkgdeck_allowed = granted;
    if (pkgdeck_mac_event) pkgdeck_mac_event(granted ? PKGDECK_ALLOWED : PKGDECK_DENIED);
}

void pkgdeck_mac_request_permission(void) {
    if (!pkgdeck_mac_bundled()) return;
    [[UNUserNotificationCenter currentNotificationCenter]
        requestAuthorizationWithOptions:(UNAuthorizationOptionAlert | UNAuthorizationOptionSound | UNAuthorizationOptionBadge)
                      completionHandler:^(BOOL granted, __unused NSError *error) { pkgdeck_set_allowed(granted); }];
}

void pkgdeck_mac_refresh_permission(void) {
    if (!pkgdeck_mac_bundled()) return;
    [[UNUserNotificationCenter currentNotificationCenter] getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
        pkgdeck_set_allowed(settings.authorizationStatus == UNAuthorizationStatusAuthorized
                            || settings.authorizationStatus == UNAuthorizationStatusProvisional);
    }];
}

void pkgdeck_mac_start(void (*callback)(int)) {
    pkgdeck_mac_event = callback;
    if (!pkgdeck_mac_bundled()) return;
    pkgdeck_notification_delegate = [[PkgDeckNotificationDelegate alloc] init];
    // The center keeps only a weak reference; this one lives as long as the app.
    [UNUserNotificationCenter currentNotificationCenter].delegate = pkgdeck_notification_delegate;
    pkgdeck_mac_request_permission();
}

void pkgdeck_mac_open_notification_settings(void) {
    NSURL *url = [NSURL URLWithString:@"x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=io.github.astrovm.PkgDeck"];
    [[NSWorkspace sharedWorkspace] openURL:url];
}

static void pkgdeck_post(NSString *title, NSString *body) {
    if (!pkgdeck_allowed) {
        // Without permission, AppleScript shows it. The text travels as
        // arguments, never as script source.
        NSTask *task = [[NSTask alloc] init];
        task.launchPath = @"/usr/bin/osascript";
        task.arguments = @[ @"-e", @"on run argv", @"-e",
                            @"display notification (item 2 of argv) with title (item 1 of argv)",
                            @"-e", @"end run", title, body ];
        @try { [task launch]; } @catch (NSException *exception) {}
        [task release];
        return;
    }
    UNMutableNotificationContent *content = [[UNMutableNotificationContent alloc] init];
    content.title = title;
    content.body = body;
    content.sound = [UNNotificationSound defaultSound];
    UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:[[NSUUID UUID] UUIDString]
                                                                          content:content
                                                                          trigger:nil];
    [content release];
    [[UNUserNotificationCenter currentNotificationCenter] addNotificationRequest:request withCompletionHandler:nil];
}

// The permission can change in System Settings at any time, so it is read
// again right before each notification.
void pkgdeck_mac_notify(const char *title, const char *body) {
    if (!pkgdeck_mac_bundled()) return;
    NSString *titleText = [[NSString stringWithUTF8String:title] copy];
    NSString *bodyText = [[NSString stringWithUTF8String:body] copy];
    [[UNUserNotificationCenter currentNotificationCenter] getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
        BOOL granted = settings.authorizationStatus == UNAuthorizationStatusAuthorized
            || settings.authorizationStatus == UNAuthorizationStatusProvisional;
        dispatch_async(dispatch_get_main_queue(), ^{
            pkgdeck_set_allowed(granted);
            pkgdeck_post(titleText, bodyText);
            [titleText release];
            [bodyText release];
        });
    }];
}

void pkgdeck_mac_set_badge(const char *label) {
    NSString *text = label[0] ? [NSString stringWithUTF8String:label] : nil;
    [[NSApp dockTile] setBadgeLabel:text];
}

// `png` is the menu bar template image at twice its 18 point size.
void pkgdeck_mac_set_tray(int visible, const unsigned char *png, unsigned long length) {
    if (!visible) {
        if (pkgdeck_item) {
            [[NSStatusBar systemStatusBar] removeStatusItem:pkgdeck_item];
            [pkgdeck_item release];
            pkgdeck_item = nil;
        }
        return;
    }
    if (pkgdeck_item) return;
    if (!pkgdeck_status_delegate) pkgdeck_status_delegate = [[PkgDeckStatusDelegate alloc] init];
    pkgdeck_item = [[[NSStatusBar systemStatusBar] statusItemWithLength:NSSquareStatusItemLength] retain];
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@"PkgDeck"];
    menu.autoenablesItems = NO;
    NSString *titles[] = {@"Open", @"Check now", @"Quit"};
    SEL actions[] = {@selector(open:), @selector(check:), @selector(quit:)};
    for (int index = 0; index < 3; index++) {
        NSMenuItem *entry = [[NSMenuItem alloc] initWithTitle:titles[index] action:actions[index] keyEquivalent:@""];
        entry.target = pkgdeck_status_delegate;
        [menu addItem:entry];
        [entry release];
    }
    pkgdeck_item.menu = menu;
    [menu release];
    NSImage *image = [[NSImage alloc] initWithData:[NSData dataWithBytes:png length:length]];
    if (image) {
        image.size = NSMakeSize(18, 18);
        [image setTemplate:YES];
    }
    if (pkgdeck_item.button) {
        pkgdeck_item.button.image = image;
        pkgdeck_item.button.imageScaling = NSImageScaleProportionallyDown;
        pkgdeck_item.button.toolTip = @"PkgDeck";
        pkgdeck_item.button.accessibilityLabel = @"PkgDeck";
    }
    [image release];
}

// Bring the app forward, as clicking its Dock icon does.
void pkgdeck_mac_activate(void) {
    [NSApp activateIgnoringOtherApps:YES];
}

// Hide the Dock icon while only the menu bar icon is left, and bring it back.
void pkgdeck_mac_set_dock_visible(int visible) {
    [NSApp setActivationPolicy:visible ? NSApplicationActivationPolicyRegular : NSApplicationActivationPolicyAccessory];
}

// While no window is open the toolkit's loop isn't running, so the menu bar
// icon's clicks are handed out here, for up to `seconds`.
void pkgdeck_mac_pump(double seconds) {
    @autoreleasepool {
        NSDate *until = [NSDate dateWithTimeIntervalSinceNow:seconds];
        for (;;) {
            NSEvent *event = [NSApp nextEventMatchingMask:NSEventMaskAny
                                                untilDate:until
                                                   inMode:NSDefaultRunLoopMode
                                                  dequeue:YES];
            if (!event) break;
            [NSApp sendEvent:event];
            until = [NSDate dateWithTimeIntervalSinceNow:0];
        }
    }
}

// Clicking the Dock icon while no window is open brings the window back.
static id pkgdeck_reopen_observer = nil;
void pkgdeck_mac_watch_reopen(int watching) {
    if (pkgdeck_reopen_observer) {
        [[NSNotificationCenter defaultCenter] removeObserver:pkgdeck_reopen_observer];
        [pkgdeck_reopen_observer release];
        pkgdeck_reopen_observer = nil;
    }
    if (!watching) return;
    pkgdeck_reopen_observer = [[[NSNotificationCenter defaultCenter]
        addObserverForName:NSApplicationDidBecomeActiveNotification
                    object:nil
                     queue:nil
                usingBlock:^(__unused NSNotification *note) { if (pkgdeck_mac_event) pkgdeck_mac_event(PKGDECK_OPEN); }] retain];
}

// The menu bar menu's item titles, one per line, or "" without the icon.
// The caller frees the string.
char *pkgdeck_mac_tray_titles(void) {
    NSMutableArray *titles = [NSMutableArray array];
    for (NSMenuItem *entry in pkgdeck_item.menu.itemArray) [titles addObject:entry.title];
    return strdup([[titles componentsJoinedByString:@"\n"] UTF8String]);
}
