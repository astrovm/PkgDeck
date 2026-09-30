#include "macos.h"

#import <AppKit/AppKit.h>
#import <UserNotifications/UserNotifications.h>

#include <QMetaObject>
#include <QBuffer>
#include <QGuiApplication>
#include <QIcon>
#include <QProcess>
#include <QPixmap>
#include <QtGlobal>
#include <utility>

@interface PkgDeckStatusDelegate : NSObject
@property(nonatomic, assign) pkgdeck::MacNative *owner;
- (void)open:(id)sender;
- (void)check:(id)sender;
- (void)quit:(id)sender;
@end

@implementation PkgDeckStatusDelegate
- (void)post:(void (*)(pkgdeck::MacNative *))signal {
    pkgdeck::MacNative *owner = self.owner;
    if (!owner)
        return;
    QMetaObject::invokeMethod(owner, [owner, signal] { signal(owner); }, Qt::QueuedConnection);
}
- (void)open:(id)sender {
    Q_UNUSED(sender);
    [self post:[](pkgdeck::MacNative *owner) { emit owner->trayOpenRequested(); }];
}
- (void)check:(id)sender {
    Q_UNUSED(sender);
    [self post:[](pkgdeck::MacNative *owner) { emit owner->trayCheckRequested(); }];
}
- (void)quit:(id)sender {
    Q_UNUSED(sender);
    [self post:[](pkgdeck::MacNative *owner) { emit owner->trayQuitRequested(); }];
}
@end

@interface PkgDeckNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@property(nonatomic, assign) pkgdeck::MacNative *owner;
@end

@implementation PkgDeckNotificationDelegate
// Show banners while PkgDeck is the active app too.
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completionHandler {
    completionHandler(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList);
}
// Callbacks arrive on a framework thread; the owner lives on Qt's.
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
    didReceiveNotificationResponse:(UNNotificationResponse *)response
             withCompletionHandler:(void (^)(void))completionHandler {
    pkgdeck::MacNative *owner = self.owner;
    QMetaObject::invokeMethod(owner, [owner] { emit owner->notificationClicked(); }, Qt::QueuedConnection);
    completionHandler();
}
@end

namespace pkgdeck {
struct MacTray {
    NSStatusItem *item;
    PkgDeckStatusDelegate *delegate;

    explicit MacTray(MacNative *owner) {
        delegate = [[PkgDeckStatusDelegate alloc] init];
        delegate.owner = owner;
        item = [[[NSStatusBar systemStatusBar] statusItemWithLength:NSSquareStatusItemLength] retain];
        NSMenu *menu = [[NSMenu alloc] initWithTitle:@"PkgDeck"];
        menu.autoenablesItems = NO;
        for (const auto &[title, action] : {
                 std::pair<NSString *, SEL>{@"Open", @selector(open:)},
                 std::pair<NSString *, SEL>{@"Check now", @selector(check:)},
                 std::pair<NSString *, SEL>{@"Quit", @selector(quit:)}}) {
            NSMenuItem *entry = [[NSMenuItem alloc] initWithTitle:title action:action keyEquivalent:@""];
            entry.target = delegate;
            [menu addItem:entry];
            [entry release];
        }
        // AppKit opens this menu itself. Qt's tray does the same and then
        // reads NSEvent.clickCount, which throws on macOS 27 unless the
        // current event is a mouse event. Nothing here reads a click count.
        item.menu = menu;
        [menu release];
        const int points = 18;
        const qreal scale = qGuiApp ? qMax(qreal(1), qGuiApp->devicePixelRatio()) : 1;
        QByteArray png;
        QBuffer buffer(&png);
        buffer.open(QIODevice::WriteOnly);
        QIcon(QStringLiteral(":/pkgdeck/logo-template.svg"))
            .pixmap(QSize(points, points) * scale)
            .toImage()
            .save(&buffer, "PNG");
        NSImage *image = [[NSImage alloc] initWithData:[NSData dataWithBytes:png.constData() length:size_t(png.size())]];
        if (image) {
            image.size = NSMakeSize(points, points);
            [image setTemplate:YES];
        }
        if (item.button) {
            item.button.image = image;
            item.button.imageScaling = NSImageScaleProportionallyDown;
            item.button.toolTip = @"PkgDeck";
            item.button.accessibilityLabel = @"PkgDeck";
        }
        [image release];
    }

    ~MacTray() {
        delegate.owner = nullptr;
        [[NSStatusBar systemStatusBar] removeStatusItem:item];
        [item release];
        [delegate release];
    }
};

MacNative::~MacNative() = default;

bool MacNative::trayAvailable() const {
    return QGuiApplication::platformName() == QStringLiteral("cocoa");
}

bool MacNative::trayVisible() const {
    return bool(tray);
}

void MacNative::setTrayVisible(bool visible) {
    if (!trayAvailable() || visible == trayVisible()) return;
    if (visible)
        tray = std::make_unique<MacTray>(this);
    else
        tray.reset();
    emit trayVisibleChanged();
}

MacNative::MacNative(QObject *parent) : QObject(parent) {
    // The notification center needs a bundle identifier; a bare binary
    // (a development build or a test) would crash asking for it.
    bundled = [[NSBundle mainBundle] bundleIdentifier] != nil;
    if (!bundled) return;
    UNUserNotificationCenter *center = [UNUserNotificationCenter currentNotificationCenter];
    PkgDeckNotificationDelegate *delegate = [[PkgDeckNotificationDelegate alloc] init];
    delegate.owner = this;
    // The center keeps only a weak reference; this one lives as long as the app.
    center.delegate = delegate;
    requestPermission();
}

void MacNative::setAllowed(bool value) {
    if (allowed == value) return;
    allowed = value;
    emit authorizedChanged();
}

void MacNative::requestPermission() {
    if (!bundled) return;
    [[UNUserNotificationCenter currentNotificationCenter]
        requestAuthorizationWithOptions:(UNAuthorizationOptionAlert | UNAuthorizationOptionSound | UNAuthorizationOptionBadge)
                      completionHandler:^(BOOL granted, NSError *error) {
                          if (error)
                              qInfo("Notification Center: %s Using AppleScript notifications instead.", error.localizedDescription.UTF8String);
                          QMetaObject::invokeMethod(this, [this, granted] { setAllowed(granted); }, Qt::QueuedConnection);
                      }];
}

void MacNative::refreshPermission() {
    if (!bundled) return;
    [[UNUserNotificationCenter currentNotificationCenter] getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
        const bool granted = settings.authorizationStatus == UNAuthorizationStatusAuthorized
            || settings.authorizationStatus == UNAuthorizationStatusProvisional;
        QMetaObject::invokeMethod(this, [this, granted] { setAllowed(granted); }, Qt::QueuedConnection);
    }];
}

void MacNative::openNotificationSettings() {
    NSURL *url = [NSURL URLWithString:@"x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=io.github.astrovm.PkgDeck"];
    [[NSWorkspace sharedWorkspace] openURL:url];
}

// The permission can change in System Settings at any time, so it is read
// again right before each notification.
void MacNative::notify(const QString &title, const QString &body) {
    if (!bundled) return;
    // The callback runs later on a framework thread. A block captures a
    // reference parameter by address, and the caller's strings are gone by
    // then, so it gets its own copies.
    const QString titleCopy = title;
    const QString bodyCopy = body;
    [[UNUserNotificationCenter currentNotificationCenter] getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
        const bool granted = settings.authorizationStatus == UNAuthorizationStatusAuthorized
            || settings.authorizationStatus == UNAuthorizationStatusProvisional;
        QMetaObject::invokeMethod(this, [this, granted, titleCopy, bodyCopy] {
            setAllowed(granted);
            post(titleCopy, bodyCopy);
        }, Qt::QueuedConnection);
    }];
}

void MacNative::post(const QString &title, const QString &body) {
    if (!allowed) {
        // The text travels as arguments, never as script source.
        QProcess::startDetached(QStringLiteral("/usr/bin/osascript"),
                                {QStringLiteral("-e"), QStringLiteral("on run argv"),
                                 QStringLiteral("-e"), QStringLiteral("display notification (item 2 of argv) with title (item 1 of argv)"),
                                 QStringLiteral("-e"), QStringLiteral("end run"), title, body});
        return;
    }
    UNMutableNotificationContent *content = [[UNMutableNotificationContent alloc] init];
    content.title = title.toNSString();
    content.body = body.toNSString();
    content.sound = [UNNotificationSound defaultSound];
    UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:[[NSUUID UUID] UUIDString]
                                                                          content:content
                                                                          trigger:nil];
    [content release];
    [[UNUserNotificationCenter currentNotificationCenter] addNotificationRequest:request withCompletionHandler:nil];
}

void MacNative::setBadge(const QString &label) {
    [[NSApp dockTile] setBadgeLabel:label.isEmpty() ? nil : label.toNSString()];
}
}
