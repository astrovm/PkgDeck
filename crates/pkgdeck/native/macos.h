#pragma once
#include <QObject>
#include <QString>

namespace pkgdeck {
// Native macOS notifications and the Dock badge. Qt's tray messages use a
// notification API macOS no longer shows, so the Mac app posts through the
// User Notifications framework instead. QML sees this object as macNative.
class MacNative : public QObject {
    Q_OBJECT
    // True inside the app bundle. Notification Center needs a signed app;
    // the ad-hoc signed build is refused, so it posts through AppleScript's
    // display notification instead (shown as Script Editor, and a click
    // doesn't open PkgDeck). Outside a bundle there is nothing to notify from.
    Q_PROPERTY(bool notificationsAllowed READ notificationsAllowed CONSTANT)
public:
    explicit MacNative(QObject *parent = nullptr);
    bool notificationsAllowed() const { return bundled; }
    Q_INVOKABLE void notify(const QString &title, const QString &body);
    // Shows text such as an update count on the Dock icon; empty clears it.
    Q_INVOKABLE void setBadge(const QString &label);
    void setAllowed(bool value);
signals:
    void notificationClicked();
private:
    // Notification Center allowed PkgDeck's own notifications.
    bool allowed = false;
    bool bundled = false;
};
}
