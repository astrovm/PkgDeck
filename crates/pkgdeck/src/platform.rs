//! The desktop around the window: the menu bar or tray icon, notifications
//! and the Dock badge. macOS uses AppKit and User Notifications
//! (`native/macos.m`); Linux uses a StatusNotifierItem and freedesktop
//! notifications over D-Bus. What the person does there arrives as
//! [`Event`]s the window reads each frame.

use crate::ui::theme;
use eframe::egui::Color32;
use std::cell::Cell;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

/// Something done outside the window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    /// **Open** in the tray menu, or a click on the tray icon.
    Open,
    /// A left click on the tray icon: show the window, or hide it if shown.
    Toggle,
    /// **Check now** in the tray menu.
    Check,
    /// **Quit** in the tray menu.
    Quit,
    /// A notification was clicked.
    NotificationClicked,
    /// macOS let PkgDeck post its own notifications, or stopped letting it.
    Permission(bool),
}

type Wake = Box<dyn Fn() + Send>;
static EVENTS: Mutex<Option<(mpsc::Sender<Event>, Wake)>> = Mutex::new(None);

pub(crate) fn send(event: Event) {
    if let Ok(guard) = EVENTS.lock() {
        if let Some((sender, wake)) = guard.as_ref() {
            let _ = sender.send(event);
            wake();
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod native {
    use std::ffi::{c_char, c_int, c_uchar, c_ulong, CString};
    unsafe extern "C" {
        fn pkgdeck_mac_start(callback: extern "C" fn(c_int));
        fn pkgdeck_mac_bundled() -> c_int;
        fn pkgdeck_mac_reduce_motion() -> c_int;
        fn pkgdeck_mac_request_permission();
        fn pkgdeck_mac_refresh_permission();
        fn pkgdeck_mac_open_notification_settings();
        fn pkgdeck_mac_notify(title: *const c_char, body: *const c_char);
        fn pkgdeck_mac_set_badge(label: *const c_char);
        fn pkgdeck_mac_set_tray(visible: c_int, png: *const c_uchar, length: c_ulong);
        fn pkgdeck_mac_activate();
        fn pkgdeck_mac_set_dock_visible(visible: c_int);
        fn pkgdeck_mac_pump(seconds: f64);
        fn pkgdeck_mac_watch_reopen(watching: c_int);
        fn pkgdeck_mac_tray_titles() -> *mut c_char;
        fn pkgdeck_mac_unify_title_bar(view: *mut std::ffi::c_void, unified: c_int) -> c_int;
        fn pkgdeck_mac_fill_window(
            view: *mut std::ffi::c_void,
            fill: c_int,
            r: c_uchar,
            g: c_uchar,
            b: c_uchar,
        );
        fn free(pointer: *mut std::ffi::c_void);
    }
    extern "C" fn event(code: c_int) {
        use super::Event::*;
        let event = match code {
            1 => Open,
            2 => Check,
            3 => Quit,
            4 => NotificationClicked,
            5 => Permission(true),
            6 => Permission(false),
            _ => return,
        };
        super::send(event);
    }
    fn text(value: &str) -> CString {
        CString::new(value.replace('\0', "")).unwrap_or_default()
    }
    pub fn start() {
        unsafe { pkgdeck_mac_start(event) }
    }
    pub fn bundled() -> bool {
        unsafe { pkgdeck_mac_bundled() != 0 }
    }
    pub fn reduce_motion() -> bool {
        unsafe { pkgdeck_mac_reduce_motion() != 0 }
    }
    pub fn request_permission() {
        unsafe { pkgdeck_mac_request_permission() }
    }
    pub fn refresh_permission() {
        unsafe { pkgdeck_mac_refresh_permission() }
    }
    pub fn open_notification_settings() {
        unsafe { pkgdeck_mac_open_notification_settings() }
    }
    pub fn notify(title: &str, body: &str) {
        let (title, body) = (text(title), text(body));
        unsafe { pkgdeck_mac_notify(title.as_ptr(), body.as_ptr()) }
    }
    pub fn set_badge(label: &str) {
        let label = text(label);
        unsafe { pkgdeck_mac_set_badge(label.as_ptr()) }
    }
    pub fn set_tray(visible: bool, png: &[u8]) {
        unsafe { pkgdeck_mac_set_tray(visible.into(), png.as_ptr(), png.len() as c_ulong) }
    }
    pub fn activate() {
        unsafe { pkgdeck_mac_activate() }
    }
    pub fn set_dock_visible(visible: bool) {
        unsafe { pkgdeck_mac_set_dock_visible(visible.into()) }
    }
    pub fn pump(seconds: f64) {
        unsafe { pkgdeck_mac_pump(seconds) }
    }
    pub fn watch_reopen(watching: bool) {
        unsafe { pkgdeck_mac_watch_reopen(watching.into()) }
    }
    /// `view` must be the window's live `NSView`, on the main thread.
    pub fn fill_window(view: std::ptr::NonNull<std::ffi::c_void>, fill: Option<[u8; 3]>) {
        let [r, g, b] = fill.unwrap_or_default();
        unsafe { pkgdeck_mac_fill_window(view.as_ptr(), fill.is_some().into(), r, g, b) }
    }
    /// `view` must be the window's live `NSView`, on the main thread.
    pub fn unify_title_bar(view: std::ptr::NonNull<std::ffi::c_void>, unified: bool) -> bool {
        unsafe { pkgdeck_mac_unify_title_bar(view.as_ptr(), unified.into()) != 0 }
    }
    pub fn tray_titles() -> Vec<String> {
        unsafe {
            let raw = pkgdeck_mac_tray_titles();
            if raw.is_null() {
                return vec![];
            }
            let text = std::ffi::CStr::from_ptr(raw).to_string_lossy().into_owned();
            free(raw.cast());
            text.lines().map(str::to_owned).collect()
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{send, Event, EVENTS};
    use futures_util::{stream, Stream, StreamExt};
    use ksni::blocking::TrayMethods;
    use std::collections::HashMap;
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;
    use zbus::message::{Message, Type};
    use zbus::wire::{OwnedVariant, Variant};
    use zbus::{Connection, MatchRule, MessageStream};

    pub struct Tray {
        pub icon: Vec<ksni::Icon>,
        /// Whether the desktop's installed icon looks the same: the
        /// logo's own blue.
        pub named: bool,
    }
    impl ksni::Tray for Tray {
        fn id(&self) -> String {
            "io.github.astrovm.PkgDeck".into()
        }
        fn title(&self) -> String {
            "PkgDeck".into()
        }
        fn icon_name(&self) -> String {
            // Desktops prefer a named icon, so another accent sends none.
            if self.named {
                "io.github.astrovm.PkgDeck".into()
            } else {
                String::new()
            }
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icon.clone()
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "PkgDeck".into(),
                ..Default::default()
            }
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            send(Event::Toggle);
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            [
                ("Open", Event::Open),
                ("Check now", Event::Check),
                ("Quit", Event::Quit),
            ]
            .into_iter()
            .map(|(label, event)| {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |_| send(event)),
                    ..Default::default()
                }
                .into()
            })
            .collect()
        }
    }
    pub type Handle = ksni::blocking::Handle<Tray>;

    const PORTAL: &str = "org.freedesktop.portal.Desktop";

    /// What the desktop's settings portal last said about movement.
    static REDUCE_MOTION: AtomicBool = AtomicBool::new(false);

    pub fn reduce_motion() -> bool {
        REDUCE_MOTION.load(Ordering::Relaxed)
    }

    /// Draws another frame, which reads the movement setting again.
    fn wake_window() {
        if let Ok(guard) = EVENTS.lock() {
            if let Some((_, wake)) = guard.as_ref() {
                wake();
            }
        }
    }

    /// Remembers what the portal said, and wakes the window when it's new.
    pub(super) fn publish_motion(reduced: bool) {
        if REDUCE_MOTION.swap(reduced, Ordering::Relaxed) != reduced {
            wake_window();
        }
    }

    /// Follows the desktop's movement setting, off the window's thread: the
    /// portal can take a while to start. Only a lost bus brings it back here.
    pub fn watch_motion() {
        static STARTED: Once = Once::new();
        STARTED.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("pkgdeck-motion".into())
                .spawn(|| loop {
                    if let Ok(connection) = zbus::block_on(Connection::session()) {
                        zbus::block_on(follow_motion(&connection, &publish_motion));
                    }
                    std::thread::sleep(super::MOTION_RECONNECT);
                });
        });
    }

    /// Reads the setting once, then again only when the portal says it
    /// changed or a new portal starts. Ends when the connection does, or
    /// right after that read if the bus won't send signals, so the caller's
    /// next try is what keeps the setting up to date.
    pub(super) async fn follow_motion(connection: &Connection, publish: &dyn Fn(bool)) {
        // Listen first, so a change during the read isn't missed.
        let signals = portal_signals(connection).await;
        if let Some(reduced) = read_motion(connection).await {
            publish(reduced);
        }
        let Ok(signals) = signals else { return };
        let mut signals = std::pin::pin!(signals);
        while let Some(message) = signals.next().await {
            if !message.is_ok_and(|message| asks_for_a_read(&message)) {
                continue;
            }
            if let Some(reduced) = read_motion(connection).await {
                publish(reduced);
            }
        }
    }

    async fn portal_signals(
        connection: &Connection,
    ) -> zbus::Result<impl Stream<Item = zbus::Result<Message>>> {
        let changes = MatchRule::builder()
            .msg_type(Type::Signal)
            .interface("org.freedesktop.portal.Settings")
            .member("SettingChanged")
            .build()?;
        let owners = MatchRule::builder()
            .msg_type(Type::Signal)
            .interface("org.freedesktop.DBus")
            .member("NameOwnerChanged")
            .arg(0, PORTAL)
            .build()?;
        let queue = NonZeroUsize::new(16);
        Ok(stream::select(
            MessageStream::for_match_rule(changes, connection, queue).await?,
            MessageStream::for_match_rule(owners, connection, queue).await?,
        ))
    }

    /// Whether a signal could change the answer. A change to any key reads
    /// them all: KDE's animation speed only signals `reduced-motion`, so the
    /// value it carries can't be trusted to keep the others right.
    fn asks_for_a_read(message: &Message) -> bool {
        let header = message.header();
        let body = message.body();
        match header.member().map(|member| member.as_str()) {
            Some("SettingChanged") => body
                .deserialize::<(String, String, OwnedVariant)>()
                .is_ok_and(|(namespace, key, _)| {
                    super::MOTION_KEYS.contains(&(namespace.as_str(), key.as_str()))
                }),
            // An empty owner means the portal went away.
            Some("NameOwnerChanged") => body
                .deserialize::<(String, String, String)>()
                .is_ok_and(|(_, _, owner)| !owner.is_empty()),
            _ => false,
        }
    }

    /// Whether the portal asks to keep movement down, or `None` when it
    /// can't say. `ReadAll` takes one call for every key, and a namespace the
    /// desktop doesn't know is just missing, where `ReadOne` makes KDE's
    /// portal log a complaint.
    async fn read_motion(connection: &Connection) -> Option<bool> {
        let namespaces = super::MOTION_KEYS.map(|(namespace, _)| namespace);
        let reply = connection
            .call_method(
                Some(PORTAL),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.Settings"),
                "ReadAll",
                &(namespaces.as_slice(),),
            )
            .await
            .ok()?;
        let body = reply.body();
        let mut values: HashMap<String, HashMap<String, OwnedVariant>> = body.deserialize().ok()?;
        Some(super::MOTION_KEYS.iter().any(|&(namespace, key)| {
            values
                .get_mut(namespace)
                .and_then(|keys| keys.remove(key))
                .and_then(|value| setting(value.into()))
                .and_then(|value| super::reduces_motion(key, &value))
                .unwrap_or(false)
        }))
    }

    /// A portal value as a [`super::Setting`], unwrapped however deep.
    pub(super) fn setting(mut value: Variant) -> Option<super::Setting> {
        while let Variant::Variant(inner) = value {
            value = *inner;
        }
        use super::Setting::*;
        Some(match value {
            Variant::Bool(on) => Flag(on),
            Variant::U8(n) => Number(n.into()),
            Variant::I32(n) => Number(n.into()),
            Variant::U32(n) => Number(n.into()),
            Variant::F64(n) => Number(n),
            Variant::Str(text) => Text(text.to_string()),
            _ => return None,
        })
    }

    pub fn spawn(icon: Vec<ksni::Icon>, named: bool) -> Option<Handle> {
        // Flatpak won't let the app own a StatusNotifierItem name, so the
        // icon registers under its connection's own name there instead.
        let sandboxed = std::path::Path::new("/.flatpak-info").exists();
        Tray { icon, named }
            .disable_dbus_name(sandboxed)
            .spawn()
            .map_err(|e| eprintln!("pkgdeck: tray icon unavailable: {e}"))
            .ok()
    }
    pub fn notify(title: &str, body: &str) {
        let (title, body) = (title.to_owned(), body.to_owned());
        std::thread::spawn(move || {
            let shown = notify_rust::Notification::new()
                .appname("PkgDeck")
                .summary(&title)
                .body(&body)
                .icon("io.github.astrovm.PkgDeck")
                .action("default", "Open")
                .timeout(notify_rust::Timeout::Milliseconds(8000))
                .show();
            if let Ok(handle) = shown {
                handle.wait_for_action(|action| {
                    if action == "default" {
                        send(Event::NotificationClicked);
                    }
                });
            }
        });
    }
}

/// How often the desktop's reduce-motion setting is read again, where
/// nothing says when it changes.
#[cfg(not(target_os = "linux"))]
const MOTION_REFRESH: Duration = Duration::from_secs(5);

/// How long to wait before connecting to the session bus again after losing
/// it, or reading again if the bus won't send signals.
#[cfg(target_os = "linux")]
const MOTION_RECONNECT: Duration = Duration::from_secs(30);

/// Desktop portal settings that can ask apps to keep movement down: the
/// standard one, GNOME's and KDE's.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MOTION_KEYS: [(&str, &str); 3] = [
    ("org.freedesktop.appearance", "reduced-motion"),
    ("org.gnome.desktop.interface", "enable-animations"),
    ("org.kde.kdeglobals.KDE", "AnimationDurationFactor"),
];

/// A desktop setting's value, as the portal sends it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq)]
enum Setting {
    Flag(bool),
    Number(f64),
    Text(String),
}

/// Whether one of [`MOTION_KEYS`] asks to keep movement down, or `None`
/// when its value makes no sense.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn reduces_motion(key: &str, value: &Setting) -> Option<bool> {
    use Setting::*;
    match (key, value) {
        // 0 has no preference, 1 asks for less.
        ("reduced-motion", Number(n)) => Some(*n == 1.0),
        ("enable-animations", Flag(on)) => Some(!on),
        // KDE scales every animation; 0 turns them off.
        ("AnimationDurationFactor", Number(n)) => Some(*n <= 0.0),
        ("AnimationDurationFactor", Text(text)) => {
            text.trim().parse::<f64>().ok().map(|n| n <= 0.0)
        }
        _ => None,
    }
}

/// The tray, notifications and badge, and the events they send.
pub struct Platform {
    events: mpsc::Receiver<Event>,
    tray_visible: bool,
    #[cfg(target_os = "linux")]
    tray: Option<linux::Handle>,
    /// The accent the Dock and tray icons are drawn in.
    icon: Color32,
    /// macOS lets PkgDeck post its own notifications.
    authorized: bool,
    /// Notifications kept instead of shown, for tests.
    recorded: Option<std::cell::RefCell<Vec<(String, String)>>>,
    /// Links kept instead of opened, for tests.
    opened: Option<std::cell::RefCell<Vec<String>>>,
    /// Whether the desktop asks to keep movement down, and when it was
    /// read.
    motion: Cell<Option<(Instant, bool)>>,
    /// Reads the desktop's setting; off until
    /// [`Self::follow_system_motion`], so tests don't depend on it.
    follows_motion: bool,
}

/// The logo as the menu bar wants it: black on clear, at twice 18 points.
#[cfg(target_os = "macos")]
const TEMPLATE_ICON: &[u8] = include_bytes!("../assets/logo-template.png");
/// The logo's own blue, which the installed icons are drawn in.
fn blue() -> Color32 {
    theme::logo_tint(crate::settings::Accent::Blue, None)
}

impl Platform {
    /// Starts listening; `wake` brings the window back to read events.
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let (sender, events) = mpsc::channel();
        if let Ok(mut guard) = EVENTS.lock() {
            *guard = Some((sender, Box::new(wake)));
        }
        #[cfg(target_os = "macos")]
        native::start();
        Self {
            events,
            tray_visible: false,
            #[cfg(target_os = "linux")]
            tray: None,
            icon: blue(),
            authorized: false,
            recorded: None,
            opened: None,
            motion: Cell::new(None),
            follows_motion: false,
        }
    }

    /// Follows the desktop's reduce-motion setting from now on.
    pub fn follow_system_motion(&mut self) {
        self.follows_motion = true;
        #[cfg(target_os = "linux")]
        linux::watch_motion();
    }

    /// Acts as if the desktop asked to keep movement down, or not.
    #[doc(hidden)]
    pub fn set_system_motion(&mut self, reduced: bool) {
        self.follows_motion = false;
        self.motion.set(Some((Instant::now(), reduced)));
    }

    /// Whether the desktop asks apps to keep movement down. It can change
    /// while PkgDeck runs: Linux hears about it, and elsewhere it's read
    /// again every few seconds.
    pub fn reduce_motion(&self) -> bool {
        #[cfg(target_os = "linux")]
        if self.follows_motion {
            return linux::reduce_motion();
        }
        #[cfg(not(target_os = "linux"))]
        if self.follows_motion
            && self
                .motion
                .get()
                .is_none_or(|(read, _)| read.elapsed() >= MOTION_REFRESH)
        {
            #[cfg(target_os = "macos")]
            let reduced = native::reduce_motion();
            #[cfg(not(target_os = "macos"))]
            let reduced = false;
            self.motion.set(Some((Instant::now(), reduced)));
        }
        self.motion.get().is_some_and(|(_, reduced)| reduced)
    }

    /// Keeps notifications instead of showing them, and says they work.
    #[doc(hidden)]
    pub fn record_notifications(&mut self) {
        self.recorded = Some(Default::default());
    }

    /// What was kept by [`Self::record_notifications`].
    #[doc(hidden)]
    pub fn recorded(&self) -> Vec<(String, String)> {
        self.recorded
            .as_ref()
            .map(|r| r.borrow().clone())
            .unwrap_or_default()
    }

    /// Keeps links instead of opening them.
    #[doc(hidden)]
    pub fn record_links(&mut self) {
        self.opened = Some(Default::default());
    }

    /// What was kept by [`Self::record_links`].
    #[doc(hidden)]
    pub fn opened(&self) -> Vec<String> {
        self.opened
            .as_ref()
            .map(|r| r.borrow().clone())
            .unwrap_or_default()
    }

    /// Opens a web link in the browser, or a `file://` folder in the file
    /// manager.
    pub fn open(&self, url: &str) {
        if let Some(opened) = &self.opened {
            opened.borrow_mut().push(url.into());
            return;
        }
        open_url(url);
    }

    /// What happened since the last call. The window's own loop delivers
    /// the menu bar icon's clicks, so this never runs the native loop:
    /// AppKit can't be re-entered while it hands the window an event.
    pub fn events(&mut self) -> Vec<Event> {
        self.take(None)
    }

    /// Like [`Self::events`], but waits up to `timeout` for the first one.
    /// With no window open, this is what keeps the tray answering.
    pub fn wait(&mut self, timeout: std::time::Duration) -> Vec<Event> {
        #[cfg(target_os = "macos")]
        native::pump(timeout.as_secs_f64());
        #[cfg(not(target_os = "macos"))]
        let first = self.events.recv_timeout(timeout).ok();
        #[cfg(target_os = "macos")]
        let first = None;
        self.take(first)
    }

    fn take(&mut self, first: Option<Event>) -> Vec<Event> {
        let events: Vec<Event> = first.into_iter().chain(self.events.try_iter()).collect();
        for event in &events {
            if let Event::Permission(allowed) = event {
                self.authorized = *allowed;
            }
        }
        events
    }

    /// Whether this desktop can show a menu bar or tray icon.
    pub fn tray_available(&self) -> bool {
        cfg!(any(target_os = "macos", target_os = "linux"))
    }

    /// Whether notifications can be shown at all.
    pub fn notifications_available(&self) -> bool {
        if self.recorded.is_some() {
            return true;
        }
        #[cfg(target_os = "macos")]
        return native::bundled();
        #[cfg(not(target_os = "macos"))]
        return cfg!(target_os = "linux");
    }

    /// macOS needs the person to allow PkgDeck's own notifications.
    pub fn permission_needed(&self) -> bool {
        #[cfg(target_os = "macos")]
        return native::bundled() && !self.authorized;
        #[cfg(not(target_os = "macos"))]
        return false;
    }

    pub fn request_permission(&self) {
        #[cfg(target_os = "macos")]
        native::request_permission();
    }

    /// Reads the permission again; it can change in System Settings.
    pub fn refresh_permission(&self) {
        #[cfg(target_os = "macos")]
        native::refresh_permission();
    }

    pub fn open_notification_settings(&self) {
        #[cfg(target_os = "macos")]
        native::open_notification_settings();
    }

    pub fn set_tray(&mut self, visible: bool) {
        if visible == self.tray_visible {
            return;
        }
        self.tray_visible = visible;
        #[cfg(target_os = "macos")]
        native::set_tray(visible, TEMPLATE_ICON);
        #[cfg(target_os = "linux")]
        {
            if visible {
                self.tray = linux::spawn(tray_icon(self.icon), self.icon == blue());
            } else if let Some(tray) = self.tray.take() {
                tray.shutdown().wait();
            }
        }
    }

    pub fn notify(&self, title: &str, body: &str) {
        if let Some(recorded) = &self.recorded {
            recorded.borrow_mut().push((title.into(), body.into()));
            return;
        }
        #[cfg(target_os = "macos")]
        native::notify(title, body);
        #[cfg(target_os = "linux")]
        linux::notify(title, body);
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = (title, body);
    }

    /// Text such as an update count on the Dock icon; "" clears it.
    pub fn set_badge(&self, label: &str) {
        #[cfg(target_os = "macos")]
        native::set_badge(label);
        #[cfg(not(target_os = "macos"))]
        let _ = label;
    }

    /// Bring the app forward after showing its window from the tray.
    pub fn activate(&self) {
        #[cfg(target_os = "macos")]
        native::activate();
    }

    /// The tray menu's items, for the startup check.
    pub fn tray_titles(&self) -> Vec<String> {
        if !self.tray_visible {
            return vec![];
        }
        #[cfg(target_os = "macos")]
        return native::tray_titles();
        #[cfg(not(target_os = "macos"))]
        return vec!["Open".into(), "Check now".into(), "Quit".into()];
    }

    /// While no window is open, opening PkgDeck again sends [`Event::Open`].
    pub fn watch_reopen(&self, watching: bool) {
        #[cfg(target_os = "macos")]
        native::watch_reopen(watching);
        #[cfg(not(target_os = "macos"))]
        let _ = watching;
    }

    /// Show or hide the Dock icon while the window hides in the menu bar.
    pub fn set_dock_visible(&self, visible: bool) {
        #[cfg(target_os = "macos")]
        {
            native::set_dock_visible(visible);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = visible;
    }

    /// Draws the Linux tray icon in the accent. macOS keeps the bundle's
    /// Dock icon and the system's black or white menu bar template.
    pub fn set_icon(&mut self, hue: Color32) {
        if hue == self.icon {
            return;
        }
        self.icon = hue;
        #[cfg(target_os = "linux")]
        if let Some(tray) = &self.tray {
            let (icon, named) = (tray_icon(hue), hue == blue());
            tray.update(move |tray| {
                tray.icon = icon;
                tray.named = named;
            });
        }
    }
}

/// Hands `url` to the desktop: `open` on a Mac, `xdg-open` elsewhere,
/// which goes through the desktop portal inside Flatpak.
fn open_url(url: &str) {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let mut command = Command::new("/usr/bin/open");
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new("xdg-open");
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // An AppImage or Snap puts its own libraries first, and the browser
    // it would start can't run with them.
    if let (Some(paths), Some(appdir)) = (
        std::env::var_os("LD_LIBRARY_PATH"),
        std::env::var_os("APPDIR"),
    ) {
        match host_libraries(&paths, std::path::Path::new(&appdir)) {
            Some(kept) => command.env("LD_LIBRARY_PATH", kept),
            None => command.env_remove("LD_LIBRARY_PATH"),
        };
    }
    match command.spawn() {
        // Waited for off the window's thread, so it leaves no zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(error) => eprintln!("pkgdeck: cannot open {url}: {error}"),
    }
}

/// `LD_LIBRARY_PATH` without the folders inside `appdir`, or `None` when
/// nothing is left.
fn host_libraries(paths: &std::ffi::OsStr, appdir: &std::path::Path) -> Option<std::ffi::OsString> {
    let kept: Vec<_> = std::env::split_paths(paths)
        .filter(|path| !path.as_os_str().is_empty() && !path.starts_with(appdir))
        .collect();
    if kept.is_empty() {
        return None;
    }
    std::env::join_paths(kept).ok()
}

/// The colour logo as the StatusNotifierItem wants it: ARGB, big-endian.
#[cfg(target_os = "linux")]
fn tray_icon(hue: Color32) -> Vec<ksni::Icon> {
    const SIZE: u32 = 64;
    let rgba = theme::logo_rgba(hue, SIZE);
    let data = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|[r, g, b, a]| [*a, *r, *g, *b])
        .collect();
    vec![ksni::Icon {
        width: SIZE as i32,
        height: SIZE as i32,
        data,
    }]
}

/// Fills the Mac window behind its content and system title bar with
/// `fill`'s colour, or makes it see-through with none.
#[cfg(target_os = "macos")]
pub fn fill_window(view: std::ptr::NonNull<std::ffi::c_void>, fill: Option<[u8; 3]>) {
    native::fill_window(view, fill);
}

/// Runs the Mac window's content up under a see-through title bar as tall
/// as a toolbar's, the system's lights in its middle, or puts the usual
/// title bar back. False while the window has no title bar to change.
#[cfg(target_os = "macos")]
pub fn unify_title_bar(view: std::ptr::NonNull<std::ffi::c_void>, unified: bool) -> bool {
    native::unify_title_bar(view, unified)
}

/// Finds out whether the desktop can blur, once, off the caller's thread,
/// and repaints `ctx` when it knows.
pub fn probe_blur(ctx: &eframe::egui::Context) {
    use crate::backdrop;
    static PROBE: std::sync::Once = std::sync::Once::new();
    backdrop::watch(ctx);
    PROBE.call_once(|| {
        if !backdrop::probe_needed() {
            return;
        }
        std::thread::spawn(move || {
            // PKGDECK_NO_BLUR turns it off, for desktops whose blur misbehaves.
            backdrop::blur_known(
                std::env::var_os("PKGDECK_NO_BLUR").is_none()
                    && desktop_blurs(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()),
            );
        });
    });
}

#[cfg(target_os = "macos")]
fn desktop_blurs(_desktop: Option<&str>) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn desktop_blurs(desktop: Option<&str>) -> bool {
    crate::backdrop::blurs_on(desktop, kwin_blur_loaded)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn desktop_blurs(_desktop: Option<&str>) -> bool {
    false
}

/// Asks KWin whether its blur effect is on.
#[cfg(target_os = "linux")]
fn kwin_blur_loaded() -> Option<bool> {
    let connection = zbus::block_on(zbus::Connection::session()).ok()?;
    let reply = zbus::block_on(connection.call_method(
        Some("org.kde.KWin"),
        "/Effects",
        Some("org.kde.kwin.Effects"),
        "isEffectLoaded",
        &("blur",),
    ))
    .ok()?;
    reply.body().deserialize::<bool>().ok()
}

/// Asks the desktop to blur behind the X11 window `window`, or to stop.
/// KWin reads this property; an empty region means the whole window, as
/// winit asks on Wayland and macOS. The few pixels outside the rounded
/// top corners blur too, too small to notice; content covers the rest.
/// One thread does the asking, so quick toggles land in order.
#[cfg(target_os = "linux")]
pub fn blur_x11(window: u32, blur: bool) {
    use std::sync::mpsc::{channel, Sender};
    static ASK: Mutex<Option<Sender<(u32, bool)>>> = Mutex::new(None);
    let Ok(mut ask) = ASK.lock() else {
        return;
    };
    let sender = ask.get_or_insert_with(|| {
        let (sender, asks) = channel();
        std::thread::spawn(move || {
            let mut connection = None;
            for (window, blur) in asks {
                if connection.is_none() {
                    connection = x11rb::connect(None).ok().map(|(connection, _)| connection);
                }
                if let Some(connection) = &connection {
                    set_blur_region(connection, window, blur);
                }
            }
        });
        sender
    });
    let _ = sender.send((window, blur));
}

#[cfg(target_os = "linux")]
fn set_blur_region(connection: &impl x11rb::connection::Connection, window: u32, blur: bool) {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let Ok(atom) = connection
        .intern_atom(false, b"_KDE_NET_WM_BLUR_BEHIND_REGION")
        .map_err(drop)
        .and_then(|cookie| cookie.reply().map_err(drop))
    else {
        return;
    };
    let _ = if blur {
        connection
            .change_property32(
                PropMode::REPLACE,
                window,
                atom.atom,
                AtomEnum::CARDINAL,
                &[],
            )
            .map(drop)
    } else {
        connection.delete_property(window, atom.atom).map(drop)
    };
    let _ = connection.flush();
}

/// The wallpaper's copy once loaded, if allowed. The first call starts
/// loading it off the caller's thread, from the cache when an earlier run
/// made it, and repaints `ctx` when it's ready.
pub fn wallpaper(
    ctx: &eframe::egui::Context,
) -> Option<std::sync::Arc<crate::backdrop::Wallpaper>> {
    use crate::backdrop;
    static LOAD: std::sync::Once = std::sync::Once::new();
    let cache = backdrop::wallpaper_cache()?;
    backdrop::watch(ctx);
    LOAD.call_once(|| {
        std::thread::spawn(move || {
            let home = std::env::var("HOME").ok().map(std::path::PathBuf::from);
            let Some(path) = home
                .as_deref()
                .and_then(wallpaper_setting)
                .and_then(|setting| backdrop::wallpaper_file(&setting))
            else {
                return;
            };
            if let Some(copy) = backdrop::load_wallpaper(&path, cache.as_deref()) {
                backdrop::wallpaper_loaded(copy);
            }
        });
    });
    backdrop::current_wallpaper()
}

/// The wallpaper the desktop names: Plasma's for the first screen, else
/// GNOME's.
fn wallpaper_setting(home: &std::path::Path) -> Option<String> {
    let plasma =
        std::fs::read_to_string(home.join(".config/plasma-org.kde.plasma.desktop-appletsrc"))
            .ok()
            .and_then(|text| crate::backdrop::plasma_wallpaper(&text));
    plasma.or_else(|| {
        let out = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.background", "picture-uri"])
            .output()
            .ok()?;
        Some(
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .trim_matches('\'')
                .to_owned(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_reach_the_window_and_wake_it() {
        // Tests start platforms in parallel and the newest one hears the
        // events, so this one claims them until a whole round arrives.
        let mut claimed = None;
        for _ in 0..200 {
            let (woke, woken) = mpsc::channel();
            let mut platform = Platform::start(move || {
                let _ = woke.send(());
            });
            send(Event::Check);
            send(Event::Permission(true));
            let first = platform.events();
            let woke_for = woken.try_iter().count();
            let drained = platform.events().is_empty();
            send(Event::Quit);
            let waited = platform.wait(std::time::Duration::from_millis(10));
            if first == vec![Event::Check, Event::Permission(true)]
                && drained
                && waited == vec![Event::Quit]
            {
                claimed = Some((platform, woke_for));
                break;
            }
        }
        let (platform, woke_for) = claimed.expect("the events never arrived");
        assert_eq!(woke_for, 2);
        assert!(platform.tray_available());
        platform.set_badge("");
        platform.refresh_permission();
    }

    #[test]
    fn links_open_without_the_bundled_libraries() {
        let appdir = std::path::Path::new("/tmp/.mount_PkgDeck");
        let kept = |paths: &str| {
            host_libraries(std::ffi::OsStr::new(paths), appdir)
                .map(|kept| kept.into_string().unwrap())
        };
        assert_eq!(kept("/tmp/.mount_PkgDeck/usr/lib"), None);
        assert_eq!(
            kept("/tmp/.mount_PkgDeck/usr/lib:/opt/cuda/lib64::/usr/local/lib"),
            Some("/opt/cuda/lib64:/usr/local/lib".into())
        );
        // A sibling folder that only starts with the same name stays.
        assert_eq!(
            kept("/tmp/.mount_PkgDeckOther/lib"),
            Some("/tmp/.mount_PkgDeckOther/lib".into())
        );
    }

    #[test]
    fn the_desktop_can_ask_to_keep_movement_down() {
        use Setting::*;
        for (key, value, reduced) in [
            ("reduced-motion", Number(1.0), Some(true)),
            ("reduced-motion", Number(0.0), Some(false)),
            ("reduced-motion", Flag(true), None),
            ("enable-animations", Flag(false), Some(true)),
            ("enable-animations", Flag(true), Some(false)),
            ("enable-animations", Text("false".into()), None),
            ("AnimationDurationFactor", Number(0.0), Some(true)),
            ("AnimationDurationFactor", Number(0.5), Some(false)),
            ("AnimationDurationFactor", Text(" 0 ".into()), Some(true)),
            ("AnimationDurationFactor", Text("1.5".into()), Some(false)),
            ("AnimationDurationFactor", Text("fast".into()), None),
            ("AnimationDurationFactor", Flag(false), None),
            ("color-scheme", Number(1.0), None),
        ] {
            assert_eq!(reduces_motion(key, &value), reduced, "{key} {value:?}");
        }
    }

    /// A settings portal on a private socket that counts every call.
    #[cfg(target_os = "linux")]
    mod fake_portal {
        use super::Setting;
        use futures_util::StreamExt;
        use std::{
            collections::HashMap,
            os::unix::net::UnixStream,
            sync::{
                atomic::{AtomicBool, AtomicUsize, Ordering},
                mpsc, Arc, Mutex,
            },
            time::Duration,
        };
        use zbus::{connection::Builder, message::Type, wire::Variant, Connection};

        pub type Values = HashMap<String, HashMap<String, Variant<'static>>>;

        pub struct Portal {
            pub connection: Connection,
            reads: Arc<AtomicUsize>,
            values: Arc<Mutex<Values>>,
            pub broken: Arc<AtomicBool>,
        }

        impl Portal {
            /// The portal, and the app's end of the connection to it.
            pub fn start() -> (Self, Connection) {
                let (ours, theirs) = UnixStream::pair().unwrap();
                let server = std::thread::spawn(move || Self::serve(ours, Values::default()));
                let client = zbus::block_on(Builder::unix_stream(theirs).p2p().build()).unwrap();
                (server.join().unwrap(), client)
            }

            /// Answers one client on `stream`, starting with `values`. It also
            /// answers what a client asks the bus, so it can stand in for one.
            pub fn serve(stream: UnixStream, values: Values) -> Self {
                // A client's first call can come right away, so this listens
                // from before the connection starts reading.
                let mut calls = zbus::block_on(
                    Builder::unix_stream(stream)
                        .server(zbus::Guid::generate())
                        .p2p()
                        .build_message_stream(),
                )
                .unwrap();
                let connection = Connection::from(&calls);
                let portal = Self {
                    connection: connection.clone(),
                    reads: Default::default(),
                    values: Arc::new(Mutex::new(values)),
                    broken: Default::default(),
                };
                let (reads, values, broken) = (
                    portal.reads.clone(),
                    portal.values.clone(),
                    portal.broken.clone(),
                );
                std::thread::spawn(move || {
                    zbus::block_on(async {
                        while let Some(Ok(call)) = calls.next().await {
                            if call.message_type() != Type::MethodCall {
                                continue;
                            }
                            let header = call.header();
                            let member = header.member().map_or("", |name| name.as_str());
                            let to_the_bus = header
                                .interface()
                                .is_some_and(|name| name.as_str() == "org.freedesktop.DBus");
                            if to_the_bus {
                                let _ = if member == "Hello" {
                                    connection.reply(&header, &":1.1").await
                                } else {
                                    connection.reply(&header, &()).await
                                };
                                continue;
                            }
                            reads.fetch_add(1, Ordering::SeqCst);
                            if member == "ReadAll" && !broken.load(Ordering::SeqCst) {
                                let answer = values.lock().unwrap().clone();
                                let _ = connection.reply(&header, &answer).await;
                            } else {
                                let _ = connection
                                    .reply_error(
                                        &header,
                                        "org.freedesktop.portal.Error.NotFound",
                                        &"Requested setting not found",
                                    )
                                    .await;
                            }
                        }
                    })
                });
                portal
            }

            pub fn reads(&self) -> usize {
                self.reads.load(Ordering::SeqCst)
            }

            /// Changes a value without telling anyone.
            pub fn set(&self, namespace: &str, key: &str, value: Setting) {
                let value = match value {
                    Setting::Flag(on) => Variant::Bool(on),
                    Setting::Number(n) => Variant::F64(n),
                    Setting::Text(text) => Variant::from(text),
                };
                self.values
                    .lock()
                    .unwrap()
                    .entry(namespace.into())
                    .or_default()
                    .insert(key.into(), value);
            }

            pub fn setting_changed(&self, namespace: &str, key: &str) {
                zbus::block_on(self.connection.emit_signal(
                    None::<&str>,
                    "/org/freedesktop/portal/desktop",
                    "org.freedesktop.portal.Settings",
                    "SettingChanged",
                    &(namespace, key, Variant::U32(0)),
                ))
                .unwrap();
            }

            pub fn owner_changed(&self, name: &str, owner: &str) {
                zbus::block_on(self.connection.emit_signal(
                    None::<&str>,
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "NameOwnerChanged",
                    &(name, "", owner),
                ))
                .unwrap();
            }
        }

        /// The next value published, which must come soon.
        pub fn next(published: &mpsc::Receiver<bool>) -> bool {
            published.recv_timeout(Duration::from_secs(10)).unwrap()
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_portal_is_read_once_and_again_only_when_it_says_something_changed() {
        use fake_portal::{next, Portal};
        use Setting::*;
        let quiet = |published: &mpsc::Receiver<bool>| {
            assert_eq!(
                published.recv_timeout(std::time::Duration::from_millis(300)),
                Err(mpsc::RecvTimeoutError::Timeout)
            );
        };
        let (portal, client) = Portal::start();
        let (publish, published) = mpsc::channel();
        let follower = std::thread::spawn(move || {
            zbus::block_on(linux::follow_motion(&client, &|reduced| {
                let _ = publish.send(reduced);
            }));
        });
        let appearance = "org.freedesktop.appearance";
        let kde = "org.kde.kdeglobals.KDE";
        // One read at the start, of everything at once.
        assert!(!next(&published));
        assert_eq!(portal.reads(), 1);
        // A signal for what PkgDeck reads brings it up to date, with one read.
        portal.set(appearance, "reduced-motion", Number(1.0));
        portal.setting_changed(appearance, "reduced-motion");
        assert!(next(&published));
        assert_eq!(portal.reads(), 2);
        // Without a signal, nothing is read, however long it waits.
        portal.set(appearance, "reduced-motion", Number(0.0));
        quiet(&published);
        assert_eq!(portal.reads(), 2);
        // Other settings changing doesn't read either: the next signal that
        // matters is the one and only read.
        portal.setting_changed(appearance, "color-scheme");
        portal.setting_changed("org.example.other", "reduced-motion");
        portal.setting_changed(appearance, "reduced-motion");
        assert!(!next(&published));
        quiet(&published);
        assert_eq!(portal.reads(), 3);
        // KDE only signals `reduced-motion` when its animation speed changes,
        // so one signal must refresh every key, not just the one it names.
        portal.set(kde, "AnimationDurationFactor", Text("0".into()));
        portal.setting_changed(kde, "AnimationDurationFactor");
        assert!(next(&published));
        portal.set(kde, "AnimationDurationFactor", Text("0.5".into()));
        portal.setting_changed(appearance, "reduced-motion");
        assert!(!next(&published));
        assert_eq!(portal.reads(), 5);
        // The portal going away reads nothing, and a failed read keeps what
        // was known. When a portal appears, it's asked again.
        portal.set(kde, "AnimationDurationFactor", Text("0".into()));
        portal.owner_changed("org.freedesktop.portal.Desktop", "");
        quiet(&published);
        assert_eq!(portal.reads(), 5);
        portal
            .broken
            .store(true, std::sync::atomic::Ordering::SeqCst);
        portal.owner_changed("org.freedesktop.portal.Desktop", ":1.2");
        portal.owner_changed("org.example.other", ":1.3");
        for _ in 0..1000 {
            if portal.reads() == 6 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(portal.reads(), 6);
        assert_eq!(published.try_recv(), Err(mpsc::TryRecvError::Empty));
        portal
            .broken
            .store(false, std::sync::atomic::Ordering::SeqCst);
        portal.owner_changed("org.freedesktop.portal.Desktop", ":1.4");
        assert!(next(&published));
        assert_eq!(portal.reads(), 7);
        // A lost connection ends it, so the caller can connect again.
        zbus::block_on(portal.connection.clone().close()).unwrap();
        follower.join().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_watcher_reads_the_portal_on_the_session_bus() {
        use std::os::unix::net::UnixListener;
        use std::process::{Command, Stdio};
        use std::time::Duration;
        use zbus::wire::Variant;
        // The watcher is one for the whole process and uses the real session
        // bus, so a separate process runs it, with a fake bus as its session.
        const CHILD: &str = "PKGDECK_TEST_WATCHER";
        if std::env::var_os(CHILD).is_some() {
            let mut platform = Platform::start(|| {});
            platform.follow_system_motion();
            for _ in 0..1000 {
                if platform.reduce_motion() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("the watcher never read the portal");
        }
        let path = std::env::temp_dir().join(format!("pkgdeck-bus-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::tests::the_watcher_reads_the_portal_on_the_session_bus",
            ])
            .env(CHILD, "1")
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", path.display()),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let values = fake_portal::Values::from([(
            "org.freedesktop.appearance".to_string(),
            std::collections::HashMap::from([("reduced-motion".to_string(), Variant::F64(1.0))]),
        )]);
        let portal = loop {
            match listener.accept() {
                Ok((stream, _)) => break Some(fake_portal::Portal::serve(stream, values)),
                // Stopping without connecting fails below.
                Err(_) if child.try_wait().unwrap().is_some() => break None,
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let finished = child.wait().unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(finished.success(), "{finished}");
        assert_eq!(portal.map(|portal| portal.reads()), Some(1));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn portal_values_become_settings() {
        use linux::setting;
        use zbus::wire::Variant;
        use Setting::*;
        assert_eq!(setting(Variant::Bool(false)), Some(Flag(false)));
        assert_eq!(setting(Variant::U8(1)), Some(Number(1.0)));
        assert_eq!(setting(Variant::I32(-1)), Some(Number(-1.0)));
        assert_eq!(setting(Variant::U32(1)), Some(Number(1.0)));
        assert_eq!(setting(Variant::F64(0.5)), Some(Number(0.5)));
        assert_eq!(setting(Variant::from("0")), Some(Text("0".into())));
        // A value can come wrapped more than once.
        let wrapped = Variant::Variant(Box::new(Variant::Variant(Box::new(Variant::U32(1)))));
        assert_eq!(setting(wrapped), Some(Number(1.0)));
        assert_eq!(setting(Variant::I64(1)), None);
    }

    #[test]
    fn movement_follows_the_desktop_only_when_asked() {
        let mut platform = Platform::start(|| {});
        assert!(!platform.reduce_motion());
        platform.set_system_motion(true);
        assert!(platform.reduce_motion());
        platform.set_system_motion(false);
        assert!(!platform.reduce_motion());
        // Linux hears about changes, and a change wakes the window once. No
        // real watcher here, so the test doesn't depend on this desktop.
        #[cfg(target_os = "linux")]
        {
            platform.follows_motion = true;
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::Arc;
            let mut heard = false;
            for reduced in [true, false].into_iter().cycle().take(400) {
                // Tests start platforms in parallel and the newest one is
                // woken, so this one tries until it hears the change.
                let wakes = Arc::new(AtomicUsize::new(0));
                let woken = wakes.clone();
                let mut newest = Platform::start(move || {
                    woken.fetch_add(1, Ordering::Relaxed);
                });
                newest.follows_motion = true;
                linux::publish_motion(reduced);
                assert_eq!(platform.reduce_motion(), reduced);
                let after_change = wakes.load(Ordering::Relaxed);
                linux::publish_motion(reduced);
                if after_change == 1 && wakes.load(Ordering::Relaxed) == 1 {
                    heard = true;
                    break;
                }
            }
            assert!(heard, "the window never heard about the change");
            linux::publish_motion(false);
            assert!(!platform.reduce_motion());
        }
        // Elsewhere, following reads the desktop now and keeps that a while.
        #[cfg(not(target_os = "linux"))]
        {
            platform.set_system_motion(true);
            platform.follow_system_motion();
            assert!(platform.reduce_motion());
            platform.motion.set(None);
            let first = platform.reduce_motion();
            assert!(platform.motion.get().is_some());
            assert_eq!(platform.reduce_motion(), first);
        }
    }
}
