//! The few Qt shapes the controller was written against, in plain Rust:
//! text, a file link, and an owned controller. Keeping their names keeps
//! the controller and its tests as they were when Qt drew the window.

use std::{fmt, ops::Deref, pin::Pin};

/// Text, as the controller's properties and arguments carry it.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QString(String);
impl QString {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for QString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl From<&str> for QString {
    fn from(text: &str) -> Self {
        Self(text.to_owned())
    }
}
impl From<String> for QString {
    fn from(text: String) -> Self {
        Self(text)
    }
}
impl From<&String> for QString {
    fn from(text: &String) -> Self {
        Self(text.clone())
    }
}

/// A link to a file the user chose.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QUrl(String);
impl QUrl {
    pub fn from_local_file(path: &QString) -> Self {
        Self(format!("file://{path}"))
    }
    /// The path of a `file://` link; other links have none.
    pub fn to_local_file(&self) -> Option<QString> {
        self.0
            .strip_prefix("file://")
            .filter(|path| path.starts_with('/'))
            .map(QString::from)
    }
}
impl From<&str> for QUrl {
    fn from(link: &str) -> Self {
        Self(link.to_owned())
    }
}

/// The one owner of a controller, which hands it out pinned as Qt did.
pub struct UniquePtr<T>(Box<T>);
impl<T: Unpin> UniquePtr<T> {
    pub fn new(value: T) -> Self {
        Self(Box::new(value))
    }
    pub fn pin_mut(&mut self) -> Pin<&mut T> {
        Pin::new(&mut self.0)
    }
}
impl<T> Deref for UniquePtr<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_file_links_behave_as_qt_did() {
        let text = QString::from("PkgDeck");
        assert_eq!(text.to_string(), "PkgDeck");
        assert_eq!(text.as_str(), "PkgDeck");
        assert!(!text.is_empty());
        assert!(QString::default().is_empty());
        assert_eq!(
            QString::from(String::from("a")),
            QString::from(&String::from("a"))
        );
        let file = QUrl::from_local_file(&"/tmp/list.json".into());
        assert_eq!(file.to_local_file(), Some("/tmp/list.json".into()));
        assert_eq!(
            QUrl::from("https://example.invalid/list.json").to_local_file(),
            None
        );
        assert_eq!(QUrl::from("file://relative").to_local_file(), None);
        let mut owned = UniquePtr::new(1);
        *owned.pin_mut() += 1;
        assert_eq!(*owned, 2);
    }
}
