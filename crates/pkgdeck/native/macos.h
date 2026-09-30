#pragma once
#include <QObject>
#include <QString>
#include <memory>

namespace pkgdeck {
struct MacTray;
// Native macOS menu bar, notifications and the Dock badge. Qt's tray messages use a
// notification API macOS no longer shows, so the Mac app posts through the
// User Notifications framework instead. QML sees this object as macNative.
class MacNative : public QObject {
    Q_OBJECT
    // True inside the app bundle. Outside a bundle there is nothing to
    // notify from.
    Q_PROPERTY(bool notificationsAllowed READ notificationsAllowed CONSTANT)
    // True once macOS lets PkgDeck post its own notifications. The ad-hoc
    // signed app starts without that permission: until the person allows it
    // (the system prompt, or System Settings > Notifications), notifications
    // go through AppleScript's display notification, which shows Script
    // Editor's icon and doesn't open PkgDeck when clicked.
    Q_PROPERTY(bool authorized READ authorized NOTIFY authorizedChanged)
    Q_PROPERTY(bool trayAvailable READ trayAvailable CONSTANT)
    Q_PROPERTY(bool trayVisible READ trayVisible WRITE setTrayVisible NOTIFY trayVisibleChanged)
public:
    explicit MacNative(QObject *parent = nullptr);
    ~MacNative() override;
    bool trayAvailable() const;
    bool trayVisible() const;
    // QML's Binding calls this. A plain method is invisible to that writer.
    Q_INVOKABLE void setTrayVisible(bool visible);
    // Opens the status menu and closes it. Returns the item titles, one per
    // line, or a short reason the menu did not open. The smoke test calls
    // this while the current event is not a mouse event: Qt's tray reads
    // NSEvent.clickCount there, and that read aborts on macOS 27.
    Q_INVOKABLE QString exerciseTrayMenu();
    bool notificationsAllowed() const { return bundled; }
    bool authorized() const { return allowed; }
    Q_INVOKABLE void notify(const QString &title, const QString &body);
    // Asks macOS for permission again. It shows the prompt only while the
    // person hasn't decided; either way `authorized` follows the answer.
    Q_INVOKABLE void requestPermission();
    // Reads the current permission, which can change in System Settings
    // while PkgDeck runs.
    Q_INVOKABLE void refreshPermission();
    Q_INVOKABLE void openNotificationSettings();
    // Shows text such as an update count on the Dock icon; empty clears it.
    Q_INVOKABLE void setBadge(const QString &label);
    void setAllowed(bool value);
signals:
    void trayOpenRequested();
    void trayCheckRequested();
    void trayQuitRequested();
    void trayVisibleChanged();
    void notificationClicked();
    void authorizedChanged();
private:
    void post(const QString &title, const QString &body);
    // Notification Center allowed PkgDeck's own notifications.
    bool allowed = false;
    bool bundled = false;
    std::unique_ptr<MacTray> tray;
};
}
