//! What the window remembers between runs, in `~/.config/pkgdeck/window.json`.
//! The Qt app kept the same choices in Qt's settings (an INI file on Linux,
//! the app's preferences on macOS); the first run reads them from there.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Light, dark, or whatever the system shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub appearance: Appearance,
    pub reduce_motion: bool,
    /// The sources the list reads, as the controller's comma list; empty is
    /// all of them.
    pub source_list: String,
    pub sort_column: String,
    pub sort_ascending: bool,
    /// Keep checking for updates from the menu bar or tray once the window
    /// closes.
    pub background_mode: bool,
    pub check_interval: i32,
    pub auto_update: bool,
    pub allow_removals: bool,
    pub system_approval: String,
    pub autostart: bool,
    pub notification_history: String,
    pub last_background_state: String,
    pub sidebar_width: f32,
    /// The share of the page the details take, 0 for the default.
    pub details_width: f32,
    pub flatpak_scope_choices: String,
    pub search_hint: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::System,
            reduce_motion: false,
            source_list: String::new(),
            sort_column: String::new(),
            sort_ascending: true,
            background_mode: true,
            check_interval: 30,
            auto_update: false,
            allow_removals: true,
            system_approval: String::new(),
            autostart: false,
            notification_history: "{}".into(),
            last_background_state: "{}".into(),
            sidebar_width: 212.0,
            details_width: 0.0,
            flatpak_scope_choices: "{}".into(),
            search_hint: String::new(),
        }
    }
}

/// Where the settings live: `$XDG_CONFIG_HOME/pkgdeck`, else
/// `~/.config/pkgdeck`.
pub fn directory(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    match var("XDG_CONFIG_HOME").filter(|dir| dir.starts_with('/')) {
        Some(dir) => Some(PathBuf::from(dir).join("pkgdeck")),
        None => var("HOME")
            .filter(|home| home.starts_with('/'))
            .map(|home| PathBuf::from(home).join(".config/pkgdeck")),
    }
}

/// The Qt app's keys under `[Browser]` and the fields they fill.
fn from_qt(values: &Map<String, Value>) -> Settings {
    let mut settings = Settings::default();
    let text = |key: &str| match values.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    };
    let flag = |key: &str| text(key).map(|value| matches!(value.as_str(), "true" | "1"));
    let number = |key: &str| text(key).and_then(|value| value.parse::<i32>().ok());
    if let Some(value) = number("appearance") {
        settings.appearance = match value {
            1 => Appearance::Dark,
            2 => Appearance::Light,
            _ => Appearance::System,
        };
    }
    for (key, field) in [
        ("reduceMotion", &mut settings.reduce_motion),
        ("sortAscending", &mut settings.sort_ascending),
        ("backgroundMode", &mut settings.background_mode),
        ("autoUpdate", &mut settings.auto_update),
        ("allowRemovals", &mut settings.allow_removals),
        ("autostart", &mut settings.autostart),
    ] {
        if let Some(value) = flag(key) {
            *field = value;
        }
    }
    for (key, field) in [
        ("sourceList", &mut settings.source_list),
        ("sortColumn", &mut settings.sort_column),
        ("systemApproval", &mut settings.system_approval),
        ("notificationHistory", &mut settings.notification_history),
        ("lastBackgroundState", &mut settings.last_background_state),
        ("flatpakScopeChoices", &mut settings.flatpak_scope_choices),
        ("searchHint", &mut settings.search_hint),
    ] {
        if let Some(value) = text(key) {
            *field = value;
        }
    }
    if let Some(minutes) = number("checkInterval").filter(|minutes| *minutes > 0) {
        settings.check_interval = minutes;
    }
    if let Some(width) = number("sidebarWidth").filter(|width| *width > 0) {
        settings.sidebar_width = width as f32;
    }
    settings
}

/// Qt's INI file: `[Browser]` then `key=value` lines. Text with commas or
/// quotes is written in double quotes with backslash escapes.
fn parse_ini(text: &str) -> Map<String, Value> {
    let mut values = Map::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = name.to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = match section.as_str() {
            "General" => key.trim().to_owned(),
            _ => format!("{section}/{}", key.trim()),
        };
        let Some(key) = key.strip_prefix("Browser/") else {
            continue;
        };
        values.insert(key.to_owned(), Value::String(unquote(value.trim())));
    }
    values
}

fn unquote(value: &str) -> String {
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return value.to_owned();
    };
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// The macOS preferences, `Browser.key` entries, as `plutil` prints them.
#[cfg(any(target_os = "macos", test))]
fn parse_plist_json(text: &str) -> Map<String, Value> {
    let Ok(Value::Object(all)) = serde_json::from_str::<Value>(text) else {
        return Map::new();
    };
    all.into_iter()
        .filter_map(|(key, value)| Some((key.strip_prefix("Browser.")?.to_owned(), value)))
        .collect()
}

/// What the Qt app saved, if anything.
fn legacy(directory: &Path, home: Option<&Path>) -> Option<Settings> {
    let ini = std::fs::read_to_string(directory.join("PkgDeck.conf")).ok();
    if let Some(values) = ini.map(|text| parse_ini(&text)).filter(|v| !v.is_empty()) {
        return Some(from_qt(&values));
    }
    mac_preferences(home?)
}

/// The Qt app's preferences on macOS, through `plutil`.
#[cfg(target_os = "macos")]
fn mac_preferences(home: &Path) -> Option<Settings> {
    let plist = home.join("Library/Preferences/io.github.astrovm.PkgDeck.plist");
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(&plist)
        .output()
        .ok()?;
    let values = parse_plist_json(&String::from_utf8_lossy(&output.stdout));
    (!values.is_empty()).then(|| from_qt(&values))
}
#[cfg(not(target_os = "macos"))]
fn mac_preferences(_: &Path) -> Option<Settings> {
    None
}

/// Reads and saves the settings in one directory.
pub struct Store {
    file: Option<PathBuf>,
    saved: Settings,
}

impl Store {
    /// The settings in `directory`, the Qt app's if there are none yet, or
    /// the defaults. Without a directory nothing is saved.
    pub fn open(directory: Option<PathBuf>, home: Option<&Path>) -> (Self, Settings) {
        let Some(directory) = directory else {
            return (
                Self {
                    file: None,
                    saved: Settings::default(),
                },
                Settings::default(),
            );
        };
        let file = directory.join("window.json");
        let settings = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .or_else(|| legacy(&directory, home))
            .unwrap_or_default();
        (
            Self {
                file: Some(file),
                saved: Settings::default(),
            },
            settings,
        )
    }

    /// Writes `settings` when they changed since the last save. The file is
    /// replaced whole, so a crash halfway leaves the old one.
    pub fn save(&mut self, settings: &Settings) {
        if *settings == self.saved {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        let text = serde_json::to_string_pretty(settings).expect("settings are plain data");
        let partial = file.with_extension("json.partial");
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&partial, text))
            .and_then(|()| std::fs::rename(&partial, file));
        if written.is_ok() {
            self.saved = settings.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_follows_xdg_then_home() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(
            directory(env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/a")])),
            Some("/xdg/pkgdeck".into())
        );
        assert_eq!(
            directory(env(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/a")])),
            Some("/home/a/.config/pkgdeck".into())
        );
        assert_eq!(directory(env(&[("HOME", "")])), None);
        assert_eq!(directory(env(&[])), None);
    }

    #[test]
    fn the_qt_ini_file_carries_over() {
        let values = parse_ini(
            "[General]\nother=1\n\n[Browser]\nappearance=2\nreduceMotion=true\n\
             checkInterval=360\nsourceList=\"apt,flatpak\"\n\
             systemApproval=sudoers:automatic-updates\n\
             notificationHistory=\"{\\\"notified\\\":{}}\"\nsidebarWidth=0\n\
             autoUpdate=false\nbroken line\n",
        );
        let settings = from_qt(&values);
        assert_eq!(settings.appearance, Appearance::Light);
        assert!(settings.reduce_motion);
        assert!(!settings.auto_update);
        assert_eq!(settings.check_interval, 360);
        assert_eq!(settings.source_list, "apt,flatpak");
        assert_eq!(settings.system_approval, "sudoers:automatic-updates");
        assert_eq!(settings.notification_history, "{\"notified\":{}}");
        // Nonsense keeps the default.
        assert_eq!(settings.sidebar_width, 212.0);
    }

    #[test]
    fn the_mac_preferences_carry_over() {
        let values = parse_plist_json(
            r#"{"Browser.appearance":1,"Browser.autoUpdate":true,"Browser.checkInterval":0,
                "Browser.searchHint":"Searches 17 sources as you type.","Other":1}"#,
        );
        let settings = from_qt(&values);
        assert_eq!(settings.appearance, Appearance::Dark);
        assert!(settings.auto_update);
        assert_eq!(settings.check_interval, 30);
        assert_eq!(settings.search_hint, "Searches 17 sources as you type.");
        assert!(parse_plist_json("not json").is_empty());
        assert!(parse_plist_json("[1]").is_empty());
    }

    #[test]
    fn settings_save_once_and_read_back() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut store, mut settings) = Store::open(Some(dir.clone()), None);
        assert_eq!(settings, Settings::default());
        settings.auto_update = true;
        settings.search_hint = "ünïcode ✓ 日本".into();
        store.save(&settings);
        let modified = std::fs::metadata(dir.join("window.json"))
            .unwrap()
            .modified()
            .unwrap();
        store.save(&settings);
        assert_eq!(
            std::fs::metadata(dir.join("window.json"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        let (_, read) = Store::open(Some(dir.clone()), None);
        assert_eq!(read, settings);
        // A damaged file falls back to the defaults.
        std::fs::write(dir.join("window.json"), "{oops").unwrap();
        assert_eq!(Store::open(Some(dir.clone()), None).1, Settings::default());
        // Missing fields take their defaults.
        std::fs::write(dir.join("window.json"), r#"{"autoUpdate":true}"#).unwrap();
        let partial = Store::open(Some(dir.clone()), None).1;
        assert!(partial.auto_update && partial.background_mode);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_qt_ini_file_is_read_when_there_is_no_new_file() {
        let dir = std::env::temp_dir().join(format!("pkgdeck-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("PkgDeck.conf"), "[Browser]\nautostart=true\n").unwrap();
        assert!(Store::open(Some(dir.clone()), None).1.autostart);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn odd_values_and_escapes_read_sensibly() {
        let values = parse_ini(
            "[Browser]\nappearance=0\nsidebarWidth=180\nsearchHint=\"a\\nb\\tc\\rd\\\"e\\\"\n",
        );
        let settings = from_qt(&values);
        assert_eq!(settings.appearance, Appearance::System);
        assert_eq!(settings.sidebar_width, 180.0);
        assert_eq!(settings.search_hint, "a\nb\tc\rd\"e");
        assert_eq!(unquote("\"trailing\\\""), "trailing");
        let values =
            parse_plist_json(r#"{"Browser.sourceList": ["apt"], "Browser.autostart": true}"#);
        let settings = from_qt(&values);
        assert_eq!(settings.source_list, "");
        assert!(settings.autostart);
    }

    #[test]
    fn mac_preferences_are_read_with_plutil() {
        let home = std::env::temp_dir().join(format!("pkgdeck-home-{}", std::process::id()));
        let prefs = home.join("Library/Preferences");
        std::fs::create_dir_all(&prefs).unwrap();
        std::fs::write(
            prefs.join("io.github.astrovm.PkgDeck.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Browser.autoUpdate</key><true/></dict></plist>"#,
        )
        .unwrap();
        let config = home.join("config");
        let read = Store::open(Some(config), Some(&home)).1;
        assert_eq!(read.auto_update, cfg!(target_os = "macos"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn without_a_directory_nothing_is_saved() {
        let (mut store, settings) = Store::open(None, None);
        store.save(&Settings {
            autostart: true,
            ..settings
        });
    }
}
