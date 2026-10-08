//! The desktop around the window: the menu bar or tray icon, notifications
//! and the Dock badge. macOS uses AppKit and User Notifications
//! (`native/macos.m`); Linux uses a StatusNotifierItem and freedesktop
//! notifications over D-Bus. What the person does there arrives as
//! [`Event`]s the window reads each frame.

use std::sync::{mpsc, Mutex};

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
    use super::{send, Event};
    use ksni::blocking::TrayMethods;

    pub struct Tray {
        pub icon: Vec<ksni::Icon>,
    }
    impl ksni::Tray for Tray {
        fn id(&self) -> String {
            "io.github.astrovm.PkgDeck".into()
        }
        fn title(&self) -> String {
            "PkgDeck".into()
        }
        fn icon_name(&self) -> String {
            "io.github.astrovm.PkgDeck".into()
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
    pub fn spawn(icon: Vec<ksni::Icon>) -> Option<Handle> {
        Tray { icon }.spawn().ok()
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

/// The tray, notifications and badge, and the events they send.
pub struct Platform {
    events: mpsc::Receiver<Event>,
    tray_visible: bool,
    #[cfg(target_os = "linux")]
    tray: Option<linux::Handle>,
    /// macOS lets PkgDeck post its own notifications.
    authorized: bool,
    /// Notifications kept instead of shown, for tests.
    recorded: Option<std::cell::RefCell<Vec<(String, String)>>>,
}

/// The logo as the menu bar wants it: black on clear, at twice 18 points.
#[cfg(target_os = "macos")]
const TEMPLATE_ICON: &[u8] = include_bytes!("../assets/logo-template.png");
/// The logo in colour for the tray, ARGB at 64 pixels.
#[cfg(target_os = "linux")]
const TRAY_ICON: &[u8] = include_bytes!("../assets/logo-64.png");

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
            authorized: false,
            recorded: None,
        }
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
                self.tray = linux::spawn(tray_icon());
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

    /// While no window is open, a click on the Dock icon sends [`Event::Open`].
    pub fn watch_reopen(&self, watching: bool) {
        #[cfg(target_os = "macos")]
        native::watch_reopen(watching);
        #[cfg(not(target_os = "macos"))]
        let _ = watching;
    }

    /// Show or hide the Dock icon while the window hides in the menu bar.
    pub fn set_dock_visible(&self, visible: bool) {
        #[cfg(target_os = "macos")]
        native::set_dock_visible(visible);
        #[cfg(not(target_os = "macos"))]
        let _ = visible;
    }
}

/// The colour logo as the StatusNotifierItem wants it: ARGB, big-endian.
#[cfg(target_os = "linux")]
fn tray_icon() -> Vec<ksni::Icon> {
    let Ok(image) = image::load_from_memory(TRAY_ICON) else {
        return vec![];
    };
    let image = image.to_rgba8();
    let (width, height) = image.dimensions();
    let data = image
        .pixels()
        .flat_map(|pixel| {
            let [r, g, b, a] = pixel.0;
            [a, r, g, b]
        })
        .collect();
    vec![ksni::Icon {
        width: width as i32,
        height: height as i32,
        data,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_reach_the_window_and_wake_it() {
        let (woke, woken) = mpsc::channel();
        let mut platform = Platform::start(move || {
            let _ = woke.send(());
        });
        send(Event::Check);
        send(Event::Permission(true));
        assert_eq!(
            platform.events(),
            vec![Event::Check, Event::Permission(true)]
        );
        assert_eq!(woken.try_iter().count(), 2);
        assert!(platform.events().is_empty());
        send(Event::Quit);
        assert_eq!(
            platform.wait(std::time::Duration::from_millis(10)),
            vec![Event::Quit]
        );
        assert!(platform.tray_available());
        platform.set_badge("");
        platform.refresh_permission();
    }
}
