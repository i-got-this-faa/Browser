//! Bookmarks: an ordered, URL-unique list stored as `bookmarks.json` in the
//! config dir. The in-memory type is pure; [`Bookmarks::load`] and
//! [`Bookmarks::save`] are the only file access.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One saved page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

/// What [`Bookmarks::toggle`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggled {
    Added,
    Removed,
}

/// All bookmarks, newest first, at most one per URL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bookmarks(Vec<Bookmark>);

/// `~/.config/strip-browser/bookmarks.json`.
pub fn bookmarks_path() -> PathBuf {
    browser_config::config_dir().join("bookmarks.json")
}

impl Bookmarks {
    pub fn iter(&self) -> impl Iterator<Item = &Bookmark> {
        self.0.iter()
    }

    pub fn contains(&self, url: &str) -> bool {
        self.0.iter().any(|b| b.url == url)
    }

    /// Remove the bookmark for `url` if present, else add it (newest first).
    pub fn toggle(&mut self, url: &str, title: &str) -> Toggled {
        if let Some(i) = self.0.iter().position(|b| b.url == url) {
            self.0.remove(i);
            return Toggled::Removed;
        }
        self.0.insert(0, Bookmark { url: url.to_string(), title: title.to_string() });
        Toggled::Added
    }

    /// Bookmarks whose title or URL contains `query` (case-insensitive),
    /// newest first. A blank query matches nothing.
    pub fn search(&self, query: &str) -> Vec<&Bookmark> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        self.0
            .iter()
            .filter(|b| b.title.to_lowercase().contains(&q) || b.url.to_lowercase().contains(&q))
            .collect()
    }

    /// The bookmark an "open this" request means: an exact URL, else the
    /// newest search hit.
    pub fn find(&self, query: &str) -> Option<&Bookmark> {
        let query = query.trim();
        self.0
            .iter()
            .find(|b| b.url == query)
            .or_else(|| self.search(query).into_iter().next())
    }

    /// Read `path`. A missing file is an empty list. An unreadable or
    /// malformed file is moved to `<path>.corrupt` (so the next save cannot
    /// destroy it) and reported as an error.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        serde_json::from_slice(&bytes).map_err(|e| {
            let aside = path.with_extension("json.corrupt");
            let moved = std::fs::rename(path, &aside).is_ok();
            anyhow::anyhow!(
                "{} is not valid bookmarks JSON ({e}){}",
                path.display(),
                if moved { format!("; moved to {}", aside.display()) } else { String::new() }
            )
        })
    }

    /// Write `path` atomically: temp file in the same directory, fsync, rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(self)?;
        let write = || -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            std::io::Write::write_all(&mut f, &json)?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        };
        write().with_context(|| format!("write {}", path.display())).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strip-bookmarks-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Bookmarks {
        let mut b = Bookmarks::default();
        b.toggle("https://www.rust-lang.org/", "Rust Programming Language");
        b.toggle("https://gpui.rs/", "GPUI");
        b
    }

    #[test]
    fn toggle_adds_newest_first_then_removes() {
        let mut b = sample();
        assert_eq!(b.iter().next().unwrap().url, "https://gpui.rs/");
        assert_eq!(b.toggle("https://gpui.rs/", "ignored"), Toggled::Removed);
        assert!(!b.contains("https://gpui.rs/"));
        assert_eq!(b.iter().count(), 1);
        assert_eq!(b.toggle("https://gpui.rs/", "GPUI"), Toggled::Added);
        assert_eq!(b.iter().count(), 2, "one entry per url");
    }

    #[test]
    fn search_matches_title_or_url_case_insensitively() {
        let b = sample();
        assert_eq!(b.search("RUST").len(), 1);
        assert_eq!(b.search("gpui.rs")[0].title, "GPUI");
        assert!(b.search("").is_empty(), "blank matches nothing");
        assert!(b.search("nope").is_empty());
    }

    #[test]
    fn find_prefers_exact_url_over_search_hit() {
        let mut b = sample();
        b.toggle("https://blog.test/gpui", "gpui.rs mirror");
        assert_eq!(b.find("https://gpui.rs/").unwrap().title, "GPUI");
        assert_eq!(b.find("gpui.rs").unwrap().url, "https://blog.test/gpui", "newest hit");
        assert!(b.find("zzz").is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("nested").join("bookmarks.json");
        let b = sample();
        b.save(&path).unwrap();
        assert_eq!(Bookmarks::load(&path).unwrap(), b);
        assert!(!path.with_extension("json.tmp").exists(), "temp file renamed away");
    }

    #[test]
    fn save_replaces_the_old_file_whole() {
        let dir = temp_dir("replace");
        let path = dir.join("bookmarks.json");
        sample().save(&path).unwrap();
        Bookmarks::default().save(&path).unwrap();
        assert_eq!(Bookmarks::load(&path).unwrap(), Bookmarks::default());
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = temp_dir("missing");
        assert_eq!(Bookmarks::load(&dir.join("bookmarks.json")).unwrap(), Bookmarks::default());
    }

    #[test]
    fn corrupt_file_is_kept_aside_and_reported() {
        let dir = temp_dir("corrupt");
        let path = dir.join("bookmarks.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err = Bookmarks::load(&path).unwrap_err().to_string();
        assert!(err.contains("not valid bookmarks JSON"), "{err}");
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(dir.join("bookmarks.json.corrupt")).unwrap(), "{ not json");
    }
}
