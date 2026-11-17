//! Screenshots that macOS saves as files never reach the clipboard. This
//! watches the screenshot folder so the capture loop can record them too.

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// A new file can show up before it is complete or tagged as a screenshot;
/// keep checking it for this long before deciding it isn't one.
const SETTLE: Duration = Duration::from_secs(10);
/// How often to look again at files that haven't settled.
const RECHECK: Duration = Duration::from_millis(400);
const MAX_BYTES: u64 = 64 << 20;
/// Formats `imaging::to_png` reads (macOS can also save PDF and HEIC).
const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp"];

pub struct Shot {
    pub path: PathBuf,
    /// When the file was created.
    pub taken: SystemTime,
}

pub struct Watcher {
    dir: PathBuf,
    /// Files created before this are left alone.
    since: SystemTime,
    /// The folder's modification time when it was last listed.
    stamp: Option<SystemTime>,
    listed: Instant,
    /// Entries already queued or ruled out.
    seen: HashSet<OsString>,
    /// New files that may still turn into screenshots.
    unsettled: bool,
    queue: VecDeque<(Instant, Shot)>,
    warned: bool,
}

impl Watcher {
    pub fn new(dir: PathBuf, since: SystemTime) -> Self {
        Watcher {
            dir,
            since,
            stamp: None,
            listed: Instant::now(),
            seen: HashSet::new(),
            unsettled: false,
            queue: VecDeque::new(),
            warned: false,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The next screenshot to record, oldest first. Cheap when nothing has
    /// changed: the folder is only listed again after its contents change.
    pub fn poll(&mut self, is_shot: impl Fn(&Path) -> bool) -> Option<Shot> {
        self.scan(&is_shot);
        match self.queue.front() {
            Some((ready, _)) if *ready <= Instant::now() => self.queue.pop_front().map(|(_, shot)| shot),
            _ => None,
        }
    }

    /// Hand back a screenshot that couldn't be read yet (macOS may still be
    /// writing it). False once it is too old to be worth another try.
    pub fn retry(&mut self, shot: Shot) -> bool {
        if shot.taken.elapsed().map(|age| age >= SETTLE).unwrap_or(true) {
            return false;
        }
        self.queue.push_back((Instant::now() + RECHECK, shot));
        true
    }

    fn scan(&mut self, is_shot: &dyn Fn(&Path) -> bool) {
        let stamp = std::fs::metadata(&self.dir).and_then(|m| m.modified()).ok();
        let recheck = self.unsettled && self.listed.elapsed() >= RECHECK;
        if stamp == self.stamp && !recheck {
            return;
        }
        self.stamp = stamp;
        self.listed = Instant::now();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) => {
                if !self.warned {
                    self.warned = true;
                    eprintln!("copypasta: can't read screenshots in {}: {e}", self.dir.display());
                }
                return;
            }
        };
        let now = SystemTime::now();
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        self.unsettled = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if self.seen.contains(&name) {
                seen.insert(name);
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let path = entry.path();
            let born = meta.created().or_else(|_| meta.modified()).unwrap_or(now);
            if !meta.is_file() || born <= self.since || !is_image_name(&path) {
                seen.insert(name);
            } else if meta.len() > 0 && meta.len() <= MAX_BYTES && is_shot(&path) {
                found.push(Shot { path, taken: born });
                seen.insert(name);
            } else if now.duration_since(born).map(|age| age < SETTLE).unwrap_or(true) {
                self.unsettled = true;
            } else {
                seen.insert(name);
            }
        }
        self.seen = seen;
        found.sort_by_key(|shot| shot.taken);
        self.queue.extend(found.into_iter().map(|shot| (self.listed, shot)));
    }
}

/// Visible files in a format we can read (macOS writes a hidden temporary
/// file first, then renames it).
fn is_image_name(path: &Path) -> bool {
    let hidden = path.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(true);
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    !hidden && EXTENSIONS.contains(&ext.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let i = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let n = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("copypasta-watch-{}-{n}-{i}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn names(w: &mut Watcher, is_shot: impl Fn(&Path) -> bool) -> Vec<String> {
        std::iter::from_fn(|| w.poll(&is_shot)).map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }
    fn tagged(p: &Path) -> bool {
        p.file_name().unwrap().to_string_lossy().starts_with("Screenshot")
    }

    #[test]
    fn finds_new_screenshots_only() {
        let dir = tempdir();
        std::fs::write(dir.join("Screenshot old.png"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let mut w = Watcher::new(dir.clone(), SystemTime::now());
        assert!(names(&mut w, tagged).is_empty(), "files from before the watch are left alone");

        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("Screenshot 1.png"), b"x").unwrap();
        std::fs::write(dir.join(".Screenshot 2.png"), b"x").unwrap();
        std::fs::write(dir.join("Screenshot 3.mov"), b"x").unwrap();
        std::fs::write(dir.join("notes.png"), b"x").unwrap();
        std::fs::write(dir.join("Screenshot empty.png"), b"").unwrap();
        assert_eq!(names(&mut w, tagged), ["Screenshot 1.png"]);
        assert!(names(&mut w, tagged).is_empty(), "each screenshot is reported once");

        // A file that is tagged or filled in a moment later is still found.
        std::thread::sleep(RECHECK + Duration::from_millis(50));
        std::fs::write(dir.join("Screenshot empty.png"), b"x").unwrap();
        let mut late = names(&mut w, |_| true);
        late.sort();
        assert_eq!(late, ["Screenshot empty.png", "notes.png"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retries_only_fresh_files() {
        let dir = tempdir();
        let mut w = Watcher::new(dir.clone(), SystemTime::UNIX_EPOCH);
        assert!(w.retry(Shot { path: dir.join("a.png"), taken: SystemTime::now() }));
        assert!(w.poll(|_| true).is_none(), "not before the recheck delay");
        std::thread::sleep(RECHECK + Duration::from_millis(50));
        assert!(w.poll(|_| true).is_some());
        assert!(!w.retry(Shot { path: dir.join("b.png"), taken: SystemTime::now() - SETTLE }));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
