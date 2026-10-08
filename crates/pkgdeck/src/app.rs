//! The window's state and everything it does that isn't drawing: talking
//! to the controller, choosing rows, starting changes, background checks
//! and notifications. It runs with or without a window: while PkgDeck sits
//! in the tray, `tick` keeps checking for updates.

use crate::{
    model::{self, *},
    platform::{Event, Platform},
    settings::{Settings, Store},
};
use pkgdeck_app::{
    controller::ffi::PackageController,
    qt::{QString, QUrl, UniquePtr},
};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    pin::Pin,
    rc::Rc,
    time::{Duration, Instant},
};

/// Which properties changed since the last look.
type Changed = Rc<RefCell<HashSet<&'static str>>>;

/// Where a change's review shows: on the open app page, or in a dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ReviewOn {
    #[default]
    Dialog,
    Page(String),
}

/// A short message at the bottom of the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub tone: Tone,
    pub action: Option<ToastAction>,
    pub shown: Instant,
    pub lasts: Duration,
    /// Which notice it shows, so hiding it dismisses that notice.
    pub from_notice: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToastAction {
    Undo,
    Restart,
}

/// How the window asked to be launched.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    pub background: bool,
    pub sudo: bool,
    pub from: Vec<String>,
    pub input: String,
    pub smoke_test: bool,
}

/// What a frame asks of the window around it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowRequest {
    #[default]
    None,
    Show,
    Hide,
    Quit,
}

pub struct App {
    pub ctl: UniquePtr<PackageController>,
    changed: Changed,
    pub settings: Settings,
    store: Store,
    pub platform: Platform,
    pub launch: Launch,

    // What the controller says.
    pub rows: Vec<Row>,
    pub details: Details,
    pub notice: Notice,
    pub progress: Progress,
    pub catalog: Vec<Source>,
    pub report: ReportState,
    pub pending: Vec<String>,
    pub activity: Vec<Activity>,
    pub background: BackgroundState,
    pub confirmation: Option<Confirmation>,
    pub opened: Option<Details>,
    pub repositories: Repositories,
    pub busy: bool,
    pub writing: bool,
    pub reading: bool,
    pub refreshing: bool,

    // What the window shows.
    pub page: Page,
    pub page_changed: Instant,
    pub query: String,
    /// The query the rows are for.
    pub result_query: String,
    pub search_due: Option<Instant>,
    pub filter: String,
    pub duplicates_only: bool,
    pub show_unavailable: bool,
    /// Sources turned on in Sources; empty is all of them.
    pub enabled: HashSet<String>,
    /// Sources a page shows, when the person narrowed it.
    pub page_sources: HashMap<Page, HashSet<String>>,
    pub sort: Option<(Column, bool)>,
    pub items: Vec<Item>,
    items_stale: bool,
    /// Rows still shown while a reload of the same list runs.
    pub retained: Option<Vec<Row>>,
    pub selected: Option<String>,
    /// The Flatpak group of the selected row, to find its other copy if
    /// this one goes away.
    selected_group: Option<String>,
    /// The selected package's page is open beside the list.
    pub page_open: bool,
    pub cursor_moved: bool,
    pub unchecked: HashSet<String>,
    pub active_rows: HashSet<String>,
    pub flashes: HashMap<String, Instant>,
    snapshot: HashMap<String, (Option<String>, Option<String>, String)>,
    quick_change: bool,
    auto_confirm: Option<String>,
    pub review_on: ReviewOn,
    pub review_on_page: bool,
    pub confirm_open: bool,
    pub opening: bool,
    pub toast: Option<Toast>,
    pub restart_toast: Option<Toast>,
    pub drawer_open: bool,
    pub expanded: HashSet<u64>,
    pub scope_choices: HashMap<String, String>,
    pub details_loaded: Instant,
    pub launch_error: String,
    /// What an installed AppImage's page shows: file, launch settings,
    /// update source. Read when its page opens and after each save.
    pub app_file: AppFile,
    pub launch_settings: LaunchSettings,
    pub update_source: UpdateSource,
    app_info_for: Option<String>,
    hover_warm: Option<(usize, Instant)>,
    warmed: Option<usize>,
    read_started: Option<Instant>,
    post_write_reload: bool,
    close_pending: bool,
    pub force_quit: bool,
    pub window_visible: bool,
    pub window_request: WindowRequest,
    /// Second launches hand their input here.
    pub listener: Option<crate::opening::Listener>,
    pub ui: crate::ui::State,
    next_check: Instant,
    restart_at: Option<Instant>,
    last_poll: Instant,
}

const POLL_BUSY: Duration = Duration::from_millis(40);
const POLL_IDLE: Duration = Duration::from_millis(200);
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(220);
const WARM_AFTER: Duration = Duration::from_millis(150);
pub const FLASH: Duration = Duration::from_secs(1);
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(300);

impl App {
    /// An app around a controller: the real one, or a synthetic one in
    /// tests.
    pub fn with_controller(
        mut ctl: UniquePtr<PackageController>,
        settings: Settings,
        store: Store,
        platform: Platform,
        launch: Launch,
    ) -> Self {
        let changed: Changed = Rc::default();
        macro_rules! watch {
            ($($on:ident: $name:literal),* $(,)?) => {$(
                let flag = changed.clone();
                ctl.pin_mut().$on(move |_| { flag.borrow_mut().insert($name); }).release();
            )*};
        }
        watch!(
            on_rows_changed: "rows",
            on_details_changed: "details",
            on_notice_changed: "notice",
            on_progress_changed: "progress",
            on_repositories_changed: "repositories",
            on_source_catalog_changed: "source_catalog",
            on_report_state_changed: "report_state",
            on_pending_sources_changed: "pending_sources",
            on_activity_changed: "activity",
            on_background_state_changed: "background_state",
            on_notification_history_changed: "notification_history",
            on_system_approval_changed: "system_approval",
            on_auto_update_result_changed: "auto_update_result",
            on_self_update_changed: "self_update",
            on_confirmation_changed: "confirmation",
            on_confirmation_data_changed: "confirmation_data",
            on_opened_changed: "opened",
            on_busy_changed: "busy",
            on_writing_changed: "writing",
            on_reading_changed: "reading",
            on_refreshing_changed: "refreshing",
        );
        let enabled = if launch.from.is_empty() {
            csv(&settings.source_list)
        } else {
            launch.from.iter().cloned().collect()
        };
        let sort = Column::from_key(&settings.sort_column).map(|c| (c, settings.sort_ascending));
        let scope_choices =
            serde_json::from_str(&settings.flatpak_scope_choices).unwrap_or_default();
        let now = Instant::now();
        let mut app = Self {
            ctl,
            changed,
            settings,
            store,
            platform,
            launch,
            rows: vec![],
            details: Details::default(),
            notice: Notice::default(),
            progress: Progress::default(),
            catalog: vec![],
            report: ReportState::default(),
            pending: vec![],
            activity: vec![],
            background: BackgroundState::default(),
            confirmation: None,
            opened: None,
            repositories: Repositories::default(),
            busy: false,
            writing: false,
            reading: false,
            refreshing: false,
            page: Page::Search,
            page_changed: now,
            query: String::new(),
            result_query: String::new(),
            search_due: None,
            filter: String::new(),
            duplicates_only: false,
            show_unavailable: false,
            enabled,
            page_sources: HashMap::new(),
            sort,
            items: vec![],
            items_stale: true,
            retained: None,
            selected: None,
            selected_group: None,
            page_open: false,
            cursor_moved: false,
            unchecked: HashSet::new(),
            active_rows: HashSet::new(),
            flashes: HashMap::new(),
            snapshot: HashMap::new(),
            quick_change: false,
            auto_confirm: None,
            review_on: ReviewOn::Dialog,
            review_on_page: false,
            confirm_open: false,
            opening: false,
            toast: None,
            restart_toast: None,
            drawer_open: false,
            expanded: HashSet::new(),
            scope_choices,
            details_loaded: now,
            launch_error: String::new(),
            app_file: AppFile::default(),
            launch_settings: LaunchSettings::default(),
            update_source: UpdateSource::default(),
            app_info_for: None,
            hover_warm: None,
            warmed: None,
            read_started: None,
            post_write_reload: false,
            close_pending: false,
            force_quit: false,
            window_visible: true,
            window_request: WindowRequest::None,
            listener: None,
            ui: crate::ui::State::default(),
            next_check: now + FIRST_CHECK,
            restart_at: None,
            last_poll: now,
        };
        app.apply_settings();
        let saved = app.settings.last_background_state.clone();
        app.background = model::parse(&saved);
        app.ctl
            .pin_mut()
            .restore_system_approval(app.settings.system_approval.as_str().into());
        app.ctl
            .pin_mut()
            .restore_notification_history(app.settings.notification_history.as_str().into());
        app.ctl.pin_mut().check_sources();
        if !app.launch.input.is_empty() {
            let input = app.launch.input.clone();
            app.open_input(&input);
        }
        app
    }

    pub fn c(&mut self) -> Pin<&mut PackageController> {
        self.ctl.pin_mut()
    }

    /// Hands the settings that drive background work to the controller.
    pub fn apply_settings(&mut self) {
        let (interval, auto, removals) = (
            self.settings.check_interval,
            self.settings.auto_update,
            self.settings.allow_removals,
        );
        self.c().set_check_interval(interval);
        self.c().set_auto_update(auto);
        self.c().set_allow_removals(removals);
    }

    pub fn save_settings(&mut self) {
        self.settings.sort_column = self
            .sort
            .map(|(c, _)| c.key().to_owned())
            .unwrap_or_default();
        self.settings.sort_ascending = self.sort.is_none_or(|(_, ascending)| ascending);
        self.settings.flatpak_scope_choices =
            serde_json::to_string(&self.scope_choices).unwrap_or_else(|_| "{}".into());
        self.store.save(&self.settings);
    }

    // -----------------------------------------------------------------------
    // Polling and reacting

    /// Does whatever is due: polls the controller, runs the search the
    /// person stopped typing, checks for updates in the background, and
    /// reacts to what changed. Returns how soon it wants to run again.
    pub fn tick(&mut self, now: Instant) -> Duration {
        for event in self.platform.events() {
            self.platform_event(event);
        }
        let interval = if self.busy { POLL_BUSY } else { POLL_IDLE };
        let polling = self.busy || *self.ctl.needs_poll();
        if polling && now.duration_since(self.last_poll) >= interval {
            self.last_poll = now;
            self.c().poll();
        }
        self.react();
        if self.search_due.is_some_and(|due| now >= due) {
            self.submit_search();
        }
        if let Some((raw, at)) = self.hover_warm {
            if now.duration_since(at) >= WARM_AFTER && self.retained.is_none() {
                self.hover_warm = None;
                if self.warmed != Some(raw) && raw < self.rows.len() {
                    self.warmed = Some(raw);
                    self.c().warm_details(raw as i32);
                }
            }
        }
        if self.settings.background_mode && now >= self.next_check {
            self.next_check = now + CHECK_EVERY;
            self.check_updates(false);
        }
        if let Some(at) = self.restart_at {
            if now >= at {
                if self.writing {
                    self.restart_at = Some(now + Duration::from_secs(5));
                } else {
                    self.restart_at = None;
                    self.restart();
                }
            }
        }
        if let Some(text) = self.auto_confirm.take() {
            if self.confirmation_text() == text {
                self.c().confirm(true);
                self.react();
            }
        }
        self.flashes.retain(|_, at| now.duration_since(*at) < FLASH);
        self.expire_toasts(now);
        self.save_settings();
        let mut wait: Duration = if self.busy || *self.ctl.needs_poll() {
            interval
        } else {
            Duration::from_secs(1)
        };
        if let Some(due) = self.search_due {
            wait = wait.min(due.saturating_duration_since(now));
        }
        if self.hover_warm.is_some() {
            wait = wait.min(WARM_AFTER);
        }
        wait.min(self.next_check.saturating_duration_since(now))
    }

    pub fn confirmation_text(&self) -> String {
        self.ctl.confirmation().to_string()
    }

    /// What a second launch sent: something to open, or "" to show the
    /// window.
    pub fn next_input(&mut self) -> Option<String> {
        self.listener.as_ref()?.inputs.try_recv().ok()
    }

    pub fn handle(&mut self, event: Event) {
        self.platform_event(event);
    }

    fn platform_event(&mut self, event: Event) {
        match event {
            Event::Open => self.window_request = WindowRequest::Show,
            Event::Toggle => {
                self.window_request = if self.window_visible {
                    WindowRequest::Hide
                } else {
                    WindowRequest::Show
                }
            }
            Event::Check => self.check_updates(true),
            Event::Quit => {
                self.force_quit = true;
                self.window_request = WindowRequest::Quit;
            }
            Event::NotificationClicked => {
                self.window_request = WindowRequest::Show;
                self.open_page(Page::Updates);
            }
            Event::Permission(_) => {}
        }
    }

    /// Reads what changed and does what each change calls for.
    pub fn react(&mut self) {
        loop {
            let changed: HashSet<&'static str> = std::mem::take(&mut *self.changed.borrow_mut());
            if changed.is_empty() {
                break;
            }
            self.react_to(&changed);
        }
    }

    fn react_to(&mut self, changed: &HashSet<&'static str>) {
        let has = |name: &str| changed.contains(name);
        let was_busy = self.busy;
        let was_writing = self.writing;
        self.busy = *self.ctl.busy();
        self.writing = *self.ctl.writing();
        self.reading = *self.ctl.reading();
        self.refreshing = *self.ctl.refreshing();
        if has("source_catalog") {
            self.catalog = model::parse(self.ctl.source_catalog().as_str());
            let hint = search_hint(
                &self.catalog,
                &self.enabled_sources(),
                &self.settings.search_hint,
            );
            self.settings.search_hint = hint;
            self.items_stale = true;
        }
        if has("report_state") {
            self.report = model::parse(self.ctl.report_state().as_str());
        }
        if has("pending_sources") {
            self.pending = model::parse(self.ctl.pending_sources().as_str());
        }
        if has("progress") {
            self.progress = model::parse(self.ctl.progress().as_str());
        }
        if has("activity") {
            self.activity = model::parse(self.ctl.activity().as_str());
        }
        if has("repositories") {
            self.repositories = model::parse(self.ctl.repositories().as_str());
        }
        if has("rows") {
            let rows: Vec<Row> = model::parse(self.ctl.rows().as_str());
            let keep_old = rows.is_empty() && self.busy && self.retained.is_some();
            if !keep_old {
                let covered = self
                    .retained
                    .as_ref()
                    .is_none_or(|old| rows.len() >= old.len() || !self.busy);
                if covered {
                    self.retained = None;
                }
                self.rows = rows;
                if !self.writing {
                    self.flash_changed_rows();
                }
            }
            self.items_stale = true;
            self.restore_selection();
        }
        if has("details") {
            self.details = model::parse(self.ctl.details().as_str());
            self.details_loaded = Instant::now();
        }
        if has("opened") {
            let text = self.ctl.opened().to_string();
            self.opened = (!text.is_empty()).then(|| model::parse(&text));
            if self.opened.is_some() {
                self.opening = false;
                self.app_info_for = None;
            }
        }
        if has("notice") {
            self.notice = model::parse(self.ctl.notice().as_str());
            self.show_notice();
        }
        if has("busy") && was_busy && !self.busy {
            self.opening = self.opening && self.confirmation.is_some();
            self.retained = None;
            self.items_stale = true;
            self.flash_changed_rows();
            self.restore_selection();
            if !self.post_write_reload && !self.writing {
                self.snapshot.clear();
            }
            if self.close_pending {
                self.close_pending = false;
                self.window_request = WindowRequest::Quit;
            }
        }
        if has("busy") && self.busy && !was_busy {
            self.read_started = Some(Instant::now());
        }
        if has("writing") {
            if self.writing && !was_writing {
                self.snapshot = self
                    .rows
                    .iter()
                    .filter(|row| row.is_package())
                    .map(|row| {
                        (
                            row.identity(),
                            (
                                row.installed.clone(),
                                row.candidate.clone(),
                                row.update.clone(),
                            ),
                        )
                    })
                    .collect();
                self.flashes.clear();
            } else if !self.writing && was_writing {
                self.active_rows.clear();
                if self.page.lists() && !self.close_pending {
                    self.post_write_reload = true;
                }
            }
        }
        if has("confirmation") || has("confirmation_data") {
            self.confirmation_changed(has("confirmation"));
        }
        if has("notification_history") {
            self.settings.notification_history = self.ctl.notification_history().to_string();
        }
        if has("system_approval") {
            self.settings.system_approval = self.ctl.system_approval().to_string();
        }
        if has("background_state") {
            self.background_changed();
        }
        if has("auto_update_result") {
            let result: AutoUpdateResult = model::parse(self.ctl.auto_update_result().as_str());
            if result.total > 0 && self.can_notify() {
                self.platform.notify("PkgDeck updates", &result.message());
            }
        }
        if has("self_update") {
            match self.ctl.self_update().as_str() {
                "automatic" => self.restart_at = Some(Instant::now() + Duration::from_secs(5)),
                "manual" => {
                    self.restart_toast = Some(Toast {
                        text: "PkgDeck was updated. Restart it to use the new version.".into(),
                        tone: Tone::Success,
                        action: Some(ToastAction::Restart),
                        shown: Instant::now(),
                        lasts: Duration::from_secs(24 * 60 * 60),
                        from_notice: false,
                    })
                }
                _ => {}
            }
        }
        if self.post_write_reload && !self.busy {
            self.post_write_reload = false;
            self.reload(true, true);
        }
    }

    fn can_notify(&self) -> bool {
        self.settings.background_mode
            && self.platform.tray_available()
            && self.platform.notifications_available()
    }

    fn background_changed(&mut self) {
        let text = self.ctl.background_state().to_string();
        if text == "{}" || text.is_empty() {
            return;
        }
        self.background = model::parse(&text);
        if let Some(last) = self.background.last_check {
            let failures: Vec<Value> = self
                .background
                .failures
                .iter()
                .map(|f| serde_json::json!({"source": f.source, "kind": f.kind}))
                .collect();
            self.settings.last_background_state = serde_json::json!({
                "last_check": last,
                "available": self.background.available,
                "failures": failures,
                "notify": false,
            })
            .to_string();
        }
        let count = self.background.available;
        self.platform.set_badge(&if count > 0 {
            count.to_string()
        } else {
            String::new()
        });
        if self.background.notify && self.can_notify() {
            let body = match count {
                1 => "1 update available".to_owned(),
                count => format!("{count} updates available"),
            };
            self.platform.notify("PkgDeck updates", &body);
            self.c().acknowledge_notification();
        }
    }

    fn confirmation_changed(&mut self, text_changed: bool) {
        let text = self.confirmation_text();
        if text.is_empty() {
            self.confirmation = None;
            self.review_on_page = false;
            self.confirm_open = false;
            return;
        }
        self.opening = false;
        let data: Confirmation = model::parse(self.ctl.confirmation_data().as_str());
        let quick = std::mem::take(&mut self.quick_change);
        self.confirmation = Some(data.clone());
        if !text_changed && (self.review_on_page || self.confirm_open) {
            return;
        }
        if quick && data.review == Some(false) {
            self.auto_confirm = Some(text);
            return;
        }
        let on_page = match &self.review_on {
            ReviewOn::Page(key) => self.page_key().as_ref() == Some(key),
            ReviewOn::Dialog => false,
        };
        if on_page && data.flatpak_ref_scope.is_empty() {
            self.review_on_page = true;
        } else {
            self.confirm_open = true;
        }
    }

    pub fn confirm(&mut self, approved: bool) {
        if !approved {
            self.active_rows.clear();
        }
        self.review_on_page = false;
        self.confirm_open = false;
        self.c().confirm(approved);
        self.react();
    }

    fn show_notice(&mut self) {
        if self.notice.is_empty() {
            if self.toast.as_ref().is_some_and(|t| t.from_notice) {
                self.toast = None;
            }
            return;
        }
        if self.notice.is_toast() {
            self.toast = Some(Toast {
                text: self.notice.title.clone(),
                tone: if self.notice.kind == "success" {
                    Tone::Success
                } else {
                    Tone::Accent
                },
                action: self.notice.can_undo().then_some(ToastAction::Undo),
                shown: Instant::now(),
                lasts: Duration::from_secs(5),
                from_notice: true,
            });
        }
    }

    fn expire_toasts(&mut self, now: Instant) {
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| now.duration_since(toast.shown) >= toast.lasts)
        {
            self.hide_toast();
        }
        if self
            .restart_toast
            .as_ref()
            .is_some_and(|toast| now.duration_since(toast.shown) >= toast.lasts)
        {
            self.restart_toast = None;
        }
    }

    pub fn hide_toast(&mut self) {
        if let Some(toast) = self.toast.take() {
            if toast.from_notice && !self.notice.is_empty() {
                self.c().dismiss_notice();
                self.react();
            }
        }
    }

    pub fn toast_action(&mut self, action: ToastAction) {
        match action {
            ToastAction::Undo => {
                let notice = self.notice.clone();
                self.hide_toast();
                let Some(target) = notice.target.as_ref() else {
                    return;
                };
                let identity = model::identity_of_row_value(target);
                if let Some(raw) = self.rows.iter().position(|row| row.identity() == identity) {
                    self.active_rows.insert(identity);
                    self.c()
                        .propose(notice.undo_action.as_str().into(), raw as i32);
                    self.react();
                }
            }
            ToastAction::Restart => {
                self.restart_toast = None;
                self.restart();
            }
        }
    }

    pub fn restart(&mut self) {
        let hidden = !self.window_visible;
        let restarted = self.c().restart_app(hidden);
        self.restarted(restarted);
    }

    /// Once the new copy started, this one quits.
    fn restarted(&mut self, restarted: bool) {
        if restarted {
            self.force_quit = true;
            self.window_request = WindowRequest::Quit;
        }
    }

    fn flash_changed_rows(&mut self) {
        if self.snapshot.is_empty() {
            return;
        }
        let now = Instant::now();
        let changed: Vec<String> = self
            .rows
            .iter()
            .filter(|row| row.is_package())
            .filter(|row| {
                self.snapshot.get(&row.identity()).is_some_and(|before| {
                    *before
                        != (
                            row.installed.clone(),
                            row.candidate.clone(),
                            row.update.clone(),
                        )
                })
            })
            .map(Row::identity)
            .collect();
        for identity in changed {
            self.flashes.insert(identity, now);
        }
        if !self.busy {
            self.snapshot.clear();
        }
    }

    // -----------------------------------------------------------------------
    // Sources

    /// The sources turned on: the chosen ones, or all known ones.
    pub fn enabled_sources(&self) -> HashSet<String> {
        if self.enabled.is_empty() {
            all_sources(&self.catalog).into_iter().collect()
        } else {
            self.enabled.clone()
        }
    }

    /// The sources this page reads.
    pub fn effective_sources(&self, page: Page) -> HashSet<String> {
        let enabled = self.enabled_sources();
        match self.page_sources.get(&page) {
            Some(chosen) => chosen.intersection(&enabled).cloned().collect(),
            None => enabled,
        }
    }

    fn sources_csv(&self, page: Page) -> String {
        if page == Page::Sources {
            return String::new();
        }
        if self.enabled.is_empty() && !self.page_sources.contains_key(&page) {
            return String::new();
        }
        let sources = self.effective_sources(page);
        let mut list: Vec<String> = all_sources(&self.catalog)
            .into_iter()
            .filter(|id| sources.contains(id))
            .collect();
        list.dedup();
        list.join(",")
    }

    pub fn available(&self, id: &str) -> bool {
        catalog_entry(&self.catalog, id).available
    }

    /// Whether a source can take part in a page.
    pub fn usable(&self, id: &str, page: Page) -> bool {
        let entry = catalog_entry(&self.catalog, id);
        entry.available
            && page
                .capability()
                .is_none_or(|capability| entry.capabilities.iter().any(|c| c == capability))
    }

    pub fn set_source_enabled(&mut self, id: &str, on: bool) {
        let mut enabled = self.enabled_sources();
        if on {
            enabled.insert(id.to_owned());
        } else {
            enabled.remove(id);
        }
        let all: HashSet<String> = all_sources(&self.catalog).into_iter().collect();
        self.enabled = if enabled == all {
            HashSet::new()
        } else {
            enabled
        };
        let mut list: Vec<String> = all_sources(&self.catalog)
            .into_iter()
            .filter(|id| self.enabled.contains(id))
            .collect();
        list.dedup();
        self.settings.source_list = list.join(",");
        self.page_sources.clear();
        self.settings.search_hint = search_hint(
            &self.catalog,
            &self.enabled_sources(),
            &self.settings.search_hint,
        );
        self.reload(false, false);
    }

    pub fn set_page_sources(&mut self, page: Page, sources: HashSet<String>) {
        let all: HashSet<String> = self
            .enabled_sources()
            .into_iter()
            .filter(|id| self.usable(id, page))
            .collect();
        if sources == all || sources.is_empty() {
            self.page_sources.remove(&page);
        } else {
            self.page_sources.insert(page, sources);
        }
        self.reload(false, false);
    }

    /// "Flatpak", "12 enabled sources", "3 sources".
    pub fn source_summary(&self, page: Page) -> String {
        match self.page_sources.get(&page) {
            Some(chosen) if chosen.len() == 1 => {
                source_name(chosen.iter().next().expect("one")).to_owned()
            }
            Some(chosen) => format!("{} sources", chosen.len()),
            None => "Filter sources".into(),
        }
    }

    pub fn check_updates(&mut self, force: bool) {
        let sources = all_sources(&self.catalog)
            .into_iter()
            .filter(|id| self.enabled_sources().contains(id))
            .collect::<Vec<_>>()
            .join(",");
        let enabled = self.settings.background_mode;
        self.c()
            .check_updates(sources.into(), enabled, false, false, force);
        self.react();
    }

    // -----------------------------------------------------------------------
    // Pages and loading

    pub fn open_page(&mut self, page: Page) {
        self.drawer_open = false;
        let changed = page != self.page;
        if changed {
            self.page = page;
            self.page_changed = Instant::now();
            self.retained = None;
            self.flashes.clear();
        }
        self.selected = None;
        self.page_open = false;
        self.unchecked.clear();
        self.search_due = None;
        match page {
            Page::Search if changed => self.reload(false, false),
            Page::Search | Page::Settings => {}
            _ => self.reload(false, false),
        }
        self.items_stale = true;
    }

    pub fn typed_query(&mut self) {
        if self.query.trim().is_empty() {
            self.submit_search();
        } else {
            self.search_due = Some(Instant::now() + SEARCH_DEBOUNCE);
        }
    }

    pub fn submit_search(&mut self) {
        self.search_due = None;
        if self.page != Page::Search {
            return;
        }
        self.sort = None;
        self.reload(false, false);
    }

    /// Reads the page's rows again. `force` skips the controller's cache;
    /// `keep_selection` keeps the open row selected.
    pub fn reload(&mut self, force: bool, keep_selection: bool) {
        let page = self.page;
        if !page.lists() {
            return;
        }
        let query = if page == Page::Search {
            self.query.trim().to_owned()
        } else {
            String::new()
        };
        let same = query == self.result_query || page != Page::Search;
        if !self.rows.is_empty() && (same || page == Page::Search) {
            self.retained = Some(self.rows.clone());
        }
        if page == Page::Search && query.is_empty() {
            self.retained = None;
            self.result_query.clear();
            self.rows.clear();
            self.items_stale = true;
            self.c()
                .load("Search".into(), "".into(), "".into(), false, false);
            self.react();
            return;
        }
        self.result_query = query.clone();
        if !keep_selection {
            self.selected = None;
            self.page_open = false;
        }
        self.unchecked.clear();
        let sources = self.sources_csv(page);
        let sudo = self.launch.sudo;
        self.c().load(
            page.name().into(),
            query.into(),
            sources.into(),
            sudo,
            force,
        );
        self.react();
        if !self.busy {
            self.retained = None;
        }
        self.items_stale = true;
    }

    pub fn retry_source(&mut self, source: &str) {
        let query = if self.page == Page::Search {
            self.query.trim().to_owned()
        } else {
            String::new()
        };
        let page = self.page.name();
        self.c()
            .retry_source(page.into(), query.into(), source.into());
        self.react();
    }

    /// The rows on screen: the retained ones while a reload runs.
    pub fn shown_rows(&self) -> &[Row] {
        self.retained.as_deref().unwrap_or(&self.rows)
    }

    /// Rebuilds the visible rows when something they depend on changed.
    pub fn refresh_items(&mut self) {
        if !self.items_stale {
            return;
        }
        self.items_stale = false;
        // Sources lists every manager, with the enabled ones first.
        let sources = Some(if self.page == Page::Sources {
            self.enabled_sources()
        } else {
            self.effective_sources(self.page)
        });
        let narrowing = self.retained.is_some() && self.page == Page::Search;
        let rows = self.shown_rows();
        let options = ViewOptions {
            page: Some(self.page),
            query: if self.page == Page::Search {
                &self.result_query
            } else {
                ""
            },
            sources: sources.as_ref(),
            filter: &self.filter,
            duplicates_only: self.duplicates_only,
            show_unavailable: self.show_unavailable,
            sort: self.sort,
            scope_choices: Some(&self.scope_choices),
        };
        let mut items = visible(rows, &options);
        if narrowing {
            let q = self.query.trim().to_lowercase();
            items.retain(|item| {
                let row = &rows[item.raw];
                format!("{} {} {}", row.name, row.display_name, row.summary)
                    .to_lowercase()
                    .contains(&q)
            });
        }
        self.items = items;
    }

    pub fn invalidate(&mut self) {
        self.items_stale = true;
    }

    // -----------------------------------------------------------------------
    // Selection

    pub fn selected_item(&self) -> Option<&Item> {
        let identity = self.selected.as_ref()?;
        let rows = self.shown_rows();
        self.items
            .iter()
            .find(|item| rows[item.raw].identity() == *identity)
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.selected_item()
            .map(|item| &self.shown_rows()[item.raw])
    }

    pub fn selected_index(&self) -> Option<usize> {
        let identity = self.selected.as_ref()?;
        let rows = self.shown_rows();
        self.items
            .iter()
            .position(|item| rows[item.raw].identity() == *identity)
    }

    /// The key of the open page: "opened", the open row's identity, or
    /// none.
    pub fn page_key(&self) -> Option<String> {
        if self.opened.is_some() {
            Some("opened".into())
        } else if self.page_open {
            self.selected.clone()
        } else {
            None
        }
    }

    /// Selects the visible row at `index`; `open` opens its page too.
    pub fn choose(&mut self, index: usize, open: bool) {
        if self.retained.is_some() {
            return;
        }
        let Some(item) = self.items.get(index) else {
            return;
        };
        let raw = item.raw;
        let identity = self.rows[raw].identity();
        let group = flatpak_group(&self.rows[raw]);
        if let Some(group) = group.clone().filter(|_| item.variants.len() > 1) {
            self.scope_choices.insert(group, identity.clone());
        }
        self.selected_group = group;
        if open {
            self.page_open = true;
        }
        self.cursor_moved = true;
        if self.selected.as_ref() != Some(&identity) {
            if self.review_on_page {
                self.confirm(false);
            }
            self.selected = Some(identity);
            self.launch_error.clear();
            self.c().select(raw as i32);
            self.react();
        }
    }

    pub fn deselect(&mut self) {
        if self.review_on_page {
            self.confirm(false);
        }
        self.selected = None;
        self.page_open = false;
    }

    pub fn close_page(&mut self) {
        if self.opened.is_some() {
            self.c().close_opened();
            self.react();
            self.opened = None;
        } else {
            self.page_open = false;
        }
        if self.review_on_page {
            self.confirm(false);
        }
    }

    pub fn hovered_row(&mut self, raw: Option<usize>) {
        match raw {
            Some(raw) if self.rows.get(raw).is_some_and(Row::is_package) => {
                if self.hover_warm.map(|(r, _)| r) != Some(raw) {
                    self.hover_warm = Some((raw, Instant::now()));
                }
            }
            _ => self.hover_warm = None,
        }
    }

    fn restore_selection(&mut self) {
        self.refresh_items();
        let Some(identity) = self.selected.clone() else {
            return;
        };
        let rows = self.shown_rows();
        let found = self
            .items
            .iter()
            .find(|item| rows[item.raw].identity() == identity);
        if let Some(item) = found {
            let raw = item.raw;
            if self.retained.is_none() && *self.ctl.details() == QString::from("{}") && !self.busy {
                self.c().select(raw as i32);
            }
            return;
        }
        // The other copy of a Flatpak whose chosen copy went away.
        if let Some(group) = self.selected_group.clone() {
            let rows = self.shown_rows();
            if let Some(index) = self
                .items
                .iter()
                .position(|item| flatpak_group(&rows[item.raw]).as_ref() == Some(&group))
            {
                let open = self.page_open;
                self.selected = None;
                self.choose(index, open);
                return;
            }
        }
        if self.page == Page::Search && self.rows.iter().any(|row| row.identity() == identity) {
            return;
        }
        if self.retained.is_none() && !self.busy {
            self.selected = None;
            self.page_open = false;
        }
    }

    /// The details that belong to the selected row, if they arrived.
    pub fn details_for_selection(&self) -> Option<&Details> {
        let selected = self.selected.as_ref()?;
        let package = self.details.package.as_ref()?;
        (package.identity() == *selected).then_some(&self.details)
    }

    /// Picks the System or User copy of a Flatpak shown as one row.
    pub fn choose_scope(&mut self, variant: usize) {
        if !self.can_act() || self.retained.is_some() {
            return;
        }
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
        let Some(&raw) = item.variants.get(variant) else {
            return;
        };
        let identity = self.rows[raw].identity();
        if let Some(group) = flatpak_group(&self.rows[raw]) {
            self.scope_choices.insert(group, identity.clone());
        }
        self.items_stale = true;
        self.refresh_items();
        self.selected = Some(identity);
        self.c().select(raw as i32);
        self.react();
        self.save_settings();
    }

    // -----------------------------------------------------------------------
    // Changes

    pub fn can_act(&self) -> bool {
        !self.busy || self.writing || self.reading
    }

    fn review_here(&mut self, identity: &str) {
        self.review_on = match self.page_key() {
            Some(key) if self.opened.is_none() && key == identity => ReviewOn::Page(key),
            _ => ReviewOn::Dialog,
        };
    }

    /// The button on a row, Enter, or the page's main button.
    pub fn run_row_action(&mut self, index: usize) {
        let Some(item) = self.items.get(index).cloned() else {
            return;
        };
        let row = self.rows[item.raw].clone();
        let identity = row.identity();
        self.review_here(&identity);
        let Some(action) = row_action(&row, self.page, &self.catalog) else {
            return;
        };
        if self.retained.is_some() || !self.can_act() {
            return;
        }
        self.active_rows.insert(identity);
        self.quick_change = true;
        self.c().propose(action.key().into(), item.raw as i32);
        self.react();
    }

    pub fn run_adopt(&mut self, index: usize) {
        let Some(item) = self.items.get(index).cloned() else {
            return;
        };
        let row = self.rows[item.raw].clone();
        self.review_here(&row.identity());
        if !can_adopt(&row, &self.catalog) || self.retained.is_some() || !self.can_act() {
            return;
        }
        self.active_rows.insert(row.identity());
        self.c().propose("adopt".into(), item.raw as i32);
        self.react();
    }

    pub fn run_page_action(&mut self) {
        if self.opened.is_some() {
            if self.can_act() {
                self.review_on = ReviewOn::Page("opened".into());
                self.quick_change = true;
                self.c().install_opened();
                self.react();
            }
            return;
        }
        if let Some(index) = self.selected_index() {
            self.run_row_action(index);
        }
    }

    /// A change by name for the selection or the whole page: shortcuts,
    /// Update all, Manage all, Refresh, Clean all.
    pub fn propose(&mut self, action: &str) {
        if self.retained.is_some() {
            return;
        }
        let whole_page = matches!(
            action,
            "upgrade-all" | "adopt-all" | "refresh" | "clean-all"
        );
        let selected = self.selected_item().cloned();
        if whole_page {
            self.review_on = ReviewOn::Dialog;
        } else if let Some(identity) = self.selected.clone() {
            self.review_here(&identity);
        }
        match action {
            "upgrade-all" => {
                let rows = &self.rows;
                self.active_rows = self.items.iter().map(|i| rows[i.raw].identity()).collect();
            }
            "adopt-all" => {
                let (rows, catalog) = (&self.rows, &self.catalog);
                self.active_rows = rows
                    .iter()
                    .filter(|row| row.source == "appimage" && can_adopt(row, catalog))
                    .map(Row::identity)
                    .collect();
            }
            _ => {
                if let Some(item) = &selected {
                    self.active_rows.insert(self.rows[item.raw].identity());
                }
            }
        }
        let index = if action == "clean-all" {
            -1
        } else {
            selected.map_or(-1, |item| item.raw as i32)
        };
        self.c().propose(action.into(), index);
        self.react();
    }

    /// Update all, or the checked ones.
    pub fn upgrade_updates(&mut self) {
        if self.unchecked.is_empty() {
            self.propose("upgrade-all");
            return;
        }
        self.review_on = ReviewOn::Dialog;
        let rows = &self.rows;
        let checked: Vec<&Row> = self
            .items
            .iter()
            .map(|item| &rows[item.raw])
            .filter(|row| row.is_package() && !self.unchecked.contains(&row.identity()))
            .collect();
        if checked.is_empty() {
            return;
        }
        let identities: Vec<Value> = checked.iter().map(|row| row.identity_value()).collect();
        self.active_rows = checked.iter().map(|row| row.identity()).collect();
        let json = Value::Array(identities).to_string();
        self.c().propose_checked(json.into());
        self.react();
    }

    pub fn cancel(&mut self) {
        self.c().cancel();
        self.react();
        self.opening = false;
    }

    pub fn open_input(&mut self, input: &str) {
        self.opening = true;
        self.c().open_input(input.into());
        self.react();
        if !self.busy && self.confirmation.is_none() {
            self.opening = false;
        }
    }

    pub fn open_file(&mut self, path: &std::path::Path) {
        let url = QUrl::from_local_file(&path.to_string_lossy().as_ref().into());
        if let Some(path) = url.to_local_file() {
            let input = path.to_string();
            self.open_input(&input);
        }
    }

    pub fn change_repository(&mut self, change: Value) {
        self.c().change_repository(change.to_string().into());
        self.react();
    }

    /// Reads an installed AppImage's file, launch settings and update source
    /// for the open page, once per row and again after a save.
    pub fn app_info(&mut self, raw: usize, identity: &str) {
        if self.app_info_for.as_deref() == Some(identity) {
            return;
        }
        self.app_info_for = Some(identity.to_owned());
        let index = raw as i32;
        self.app_file = model::parse(self.c().app_file(index).as_str());
        self.launch_settings = model::parse(self.c().app_launch_settings(index).as_str());
        self.update_source = model::parse(self.c().app_update_source(index).as_str());
    }

    pub fn reread_app_info(&mut self) {
        self.app_info_for = None;
    }

    // -----------------------------------------------------------------------
    // The window

    /// The window was asked to close: hide to the tray, wait for a running
    /// job, or quit. True when it may close now.
    pub fn close_requested(&mut self) -> bool {
        if !self.force_quit && self.settings.background_mode && self.platform.tray_available() {
            self.window_request = WindowRequest::Hide;
            return true;
        }
        if self.busy {
            self.close_pending = true;
            self.cancel();
            return false;
        }
        self.window_request = WindowRequest::Quit;
        true
    }

    pub fn quit(&mut self) {
        self.force_quit = true;
        if self.busy {
            self.close_pending = true;
            self.cancel();
        } else {
            self.window_request = WindowRequest::Quit;
        }
    }

    /// The tray icon follows background checks.
    pub fn sync_tray(&mut self) {
        let visible = self.settings.background_mode;
        self.platform.set_tray(visible);
    }

    /// Whether the "Waiting for …" line should show yet.
    pub fn waiting_shown(&self) -> bool {
        self.busy
            && !self.writing
            && self
                .read_started
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(800))
    }

    /// Read failures for this page, as the list and banners show them.
    pub fn read_failures(&self) -> Vec<Failure> {
        let sources = self.effective_sources(self.page);
        let mut failures: Vec<Failure> = if self.report.failures.is_empty() {
            self.rows
                .iter()
                .filter(|row| row.kind == "failure" && row.failure_kind != "unsupported")
                .map(|row| Failure {
                    source: row.source.clone(),
                    kind: row.failure_kind.clone(),
                    detail: Some(row.summary.clone()),
                })
                .collect()
        } else {
            self.report.failures.clone()
        };
        failures.retain(|f| {
            f.kind != "cancelled" && (self.page == Page::Sources || sources.contains(&f.source))
        });
        failures
    }

    pub fn failure_reason(&self, source: &str) -> String {
        self.rows
            .iter()
            .find(|row| row.kind == "failure" && row.source == source && !row.summary.is_empty())
            .map(|row| row.summary.clone())
            .or_else(|| {
                self.report
                    .failures
                    .iter()
                    .find(|f| f.source == source)
                    .and_then(|f| f.detail.clone())
            })
            .unwrap_or_else(|| "This source could not be checked.".into())
    }

    pub fn can_turn_off(&self, source: &str) -> bool {
        let enabled = self.enabled_sources();
        enabled.contains(source) && enabled.len() > 1
    }

    pub fn queued_count(&self) -> usize {
        self.activity.iter().filter(|a| a.state == "queued").count()
    }
}

fn csv(text: &str) -> HashSet<String> {
    text.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
