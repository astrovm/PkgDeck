#include "macos.h"

#import <AppKit/AppKit.h>
#import <UserNotifications/UserNotifications.h>

#include <QMetaObject>
#include <QProcess>
#include <QtGlobal>

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
    [center requestAuthorizationWithOptions:(UNAuthorizationOptionAlert | UNAuthorizationOptionSound | UNAuthorizationOptionBadge)
                          completionHandler:^(BOOL granted, NSError *error) {
                              if (error)
                                  qInfo("Notification Center: %s Using AppleScript notifications instead.", error.localizedDescription.UTF8String);
                              QMetaObject::invokeMethod(this, [this, granted] { setAllowed(granted); }, Qt::QueuedConnection);
                          }];
}

void MacNative::setAllowed(bool value) {
    allowed = value;
}

void MacNative::notify(const QString &title, const QString &body) {
    if (!bundled) return;
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
