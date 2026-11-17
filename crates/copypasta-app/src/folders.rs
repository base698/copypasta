//! Files that arrive in a folder instead of on the clipboard: screenshots
//! macOS saves to disk, and downloads. A thread watches those folders and
//! hands each new file to the capture loop to record as a clip.

use crate::capture::Msg;
use crate::platform;
use copypasta_core::{imaging, Captured, ClipKind, Store};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::os::unix::fs::{DirEntryExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const POLL: Duration = Duration::from_millis(150);
/// How far back files are picked up the first time.
const BACKFILL: Duration = Duration::from_secs(24 * 3600);
/// How often to check whether the screenshot folder setting changed.
const FOLDER_CHECK: Duration = Duration::from_secs(60);
/// A new file can show up before it is complete or tagged as a screenshot;
/// keep checking it for this long before deciding what it is.
const SETTLE: Duration = Duration::from_secs(10);
/// How often to look again at files that haven't settled.
const RECHECK: Duration = Duration::from_millis(400);
const MAX_SCREENSHOT_BYTES: u64 = 64 << 20;
/// A downloaded image is kept as an image up to this size, as a file and as
/// the PNG we store. Bigger ones (photos, mostly) are listed as files, so a
/// batch of them can't push the rest of the history out.
const MAX_DOWNLOAD_IMAGE_BYTES: u64 = 8 << 20;
/// Formats `imaging::to_png` reads.
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "webp"];
/// What browsers call a download that isn't finished.
const PARTIAL: &[&str] = &["crdownload", "download", "part", "partial", "opdownload", "tmp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The folder macOS saves screenshots in: only files it tagged as such.
    Screenshots,
    /// The Downloads folder: every file that arrives.
    Downloads,
}

#[derive(Debug, Clone)]
pub struct Found {
    pub path: PathBuf,
    /// When a screenshot was taken, or a download arrived.
    pub at: SystemTime,
    pub len: u64,
}

pub struct Watcher {
    dir: PathBuf,
    kind: Kind,
    /// Files from before this are left alone.
    since: SystemTime,
    /// The folder's modification time when it was last listed.
    stamp: Option<SystemTime>,
    listed: Instant,
    /// Listed at least once: what turns up from now on is seen arriving.
    started: bool,
    /// Entries already queued or ruled out, with the file each name stood
    /// for: a download can replace a placeholder of the same name.
    seen: HashMap<OsString, u64>,
    /// New files that may still turn into something to record.
    unsettled: bool,
    queue: VecDeque<(Instant, Found)>,
    warned: bool,
}

impl Watcher {
    pub fn new(dir: PathBuf, kind: Kind, since: SystemTime) -> Self {
        Watcher {
            dir,
            kind,
            since,
            stamp: None,
            listed: Instant::now(),
            started: false,
            seen: HashMap::new(),
            unsettled: false,
            queue: VecDeque::new(),
            warned: false,
        }
    }

    /// The next file to record, oldest first. Cheap when nothing has
    /// changed: the folder is only listed again after its contents change.
    /// `tagged` says whether macOS marked a file as a screenshot.
    pub fn poll(&mut self, tagged: impl Fn(&Path) -> bool) -> Option<Found> {
        self.scan(&tagged);
        match self.queue.front() {
            Some((ready, _)) if *ready <= Instant::now() => self.queue.pop_front().map(|(_, found)| found),
            _ => None,
        }
    }

    /// Hand back a file that couldn't be read yet (it may still be being
    /// written). False once it is too old to be worth another try.
    pub fn retry(&mut self, found: Found) -> bool {
        if found.at.elapsed().map(|age| age >= SETTLE).unwrap_or(true) {
            return false;
        }
        self.queue.push_back((Instant::now() + RECHECK, found));
        true
    }

    fn scan(&mut self, tagged: &dyn Fn(&Path) -> bool) {
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
                    eprintln!("copypasta: can't read {}: {e}", self.dir.display());
                }
                return;
            }
        };
        let now = SystemTime::now();
        let mut seen = HashMap::new();
        let mut found = Vec::new();
        self.unsettled = false;
        for entry in entries.flatten() {
            let (name, file) = (entry.file_name(), entry.ino());
            if self.seen.get(&name) == Some(&file) {
                seen.insert(name, file);
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let path = entry.path();
            let born = meta.created().or_else(|_| meta.modified()).unwrap_or(now);
            let at = match self.kind {
                // Seen arriving: the file dates from when the download
                // began, so go by when it got its final name.
                Kind::Downloads if self.started => born.max(changed(&meta)).min(now),
                // Already there (or a screenshot): go by when it was made.
                // Opening a file changes it too, which is not arriving.
                _ => born,
            };
            let ready = match self.kind {
                Kind::Screenshots => meta.len() <= MAX_SCREENSHOT_BYTES && tagged(&path),
                Kind::Downloads => true,
            };
            if !meta.is_file() || at <= self.since || self.kind.never_records(&path) {
                seen.insert(name, file);
            } else if meta.len() > 0 && ready {
                found.push(Found { path, at, len: meta.len() });
                seen.insert(name, file);
            } else if now.duration_since(at).map(|age| age < SETTLE).unwrap_or(true) {
                self.unsettled = true;
            } else {
                seen.insert(name, file);
            }
        }
        self.seen = seen;
        self.started = true;
        found.sort_by_key(|f| f.at);
        self.queue.extend(found.into_iter().map(|f| (self.listed, f)));
    }
}

impl Kind {
    /// Names that are never recorded: hidden files (macOS and browsers write
    /// those first, then rename them), anything but an image where only
    /// screenshots count, and downloads still in progress.
    fn never_records(self, path: &Path) -> bool {
        let hidden = path.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(true);
        hidden
            || match self {
                Kind::Screenshots => !has_extension(path, IMAGES),
                Kind::Downloads => has_extension(path, PARTIAL),
            }
    }

    fn what(self) -> &'static str {
        match self {
            Kind::Screenshots => "screenshots",
            Kind::Downloads => "downloads",
        }
    }
}

fn has_extension(path: &Path, any: &[&str]) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    any.contains(&ext.as_str())
}

/// When the file's name or attributes last changed.
fn changed(meta: &std::fs::Metadata) -> SystemTime {
    UNIX_EPOCH + Duration::new(meta.ctime().max(0) as u64, meta.ctime_nsec().clamp(0, 999_999_999) as u32)
}

/// The clip a found file makes: an image with its picture, or the file by
/// its path. None when it should be an image but can't be read as one,
/// which right after it appears usually means it is still being written.
pub fn clip_of(found: &Found, kind: Kind) -> Option<Captured> {
    let limit = match kind {
        Kind::Screenshots => MAX_SCREENSHOT_BYTES,
        Kind::Downloads => MAX_DOWNLOAD_IMAGE_BYTES,
    };
    if kind == Kind::Downloads && !(has_extension(&found.path, IMAGES) && found.len <= limit) {
        return Some(file_clip(&found.path));
    }
    let img = imaging::to_png(&std::fs::read(&found.path).ok()?).ok()?;
    if img.png.len() as u64 > limit {
        return Some(file_clip(&found.path));
    }
    Some(Captured {
        kind: ClipKind::Image,
        png: Some(img.png),
        width: Some(img.width),
        height: Some(img.height),
        path: Some(found.path.to_string_lossy().into_owned()),
        ..Default::default()
    })
}

/// A file recorded by its path, as one copied in Finder is.
pub fn file_clip(path: &Path) -> Captured {
    let path = path.to_string_lossy().into_owned();
    Captured { kind: ClipKind::File, text: Some(path.clone()), path: Some(path), ..Default::default() }
}

/// The folders to watch: where macOS saves screenshots, and Downloads. When
/// those are one folder it is watched once, as Downloads.
fn folders() -> Vec<(PathBuf, Kind)> {
    let downloads = platform::downloads_dir();
    let screenshots = platform::screenshot_dir().filter(|d| Some(d) != downloads.as_ref());
    screenshots
        .map(|d| (d, Kind::Screenshots))
        .into_iter()
        .chain(downloads.map(|d| (d, Kind::Downloads)))
        .collect()
}

/// Where to start in a folder: after the newest file from it that is
/// already recorded, so a restart picks nothing up twice, and no more than
/// a day back.
fn start_of(dir: &Path) -> SystemTime {
    let day_ago = SystemTime::now() - BACKFILL;
    let newest = Store::open().ok().and_then(|store| store.newest_file_in(dir).ok().flatten());
    newest.map(|ms| UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64 + 1)).map_or(day_ago, |t| t.max(day_ago))
}

/// Watch the folders on a thread of their own: reading them can wait on a
/// macOS permission prompt, and turning a file into a clip takes a moment,
/// neither of which should hold up the clipboard.
pub fn spawn(tx: Sender<Msg>) {
    let run = move || {
        let mut watchers: Vec<Watcher> = Vec::new();
        let mut checked: Option<Instant> = None;
        loop {
            if checked.map(|t| t.elapsed() >= FOLDER_CHECK).unwrap_or(true) {
                checked = Some(Instant::now());
                let wanted = folders();
                watchers.retain(|w| wanted.iter().any(|(dir, kind)| dir == &w.dir && *kind == w.kind));
                for (dir, kind) in wanted {
                    if !watchers.iter().any(|w| w.dir == dir && w.kind == kind) {
                        eprintln!("copypasta: watching {} for {}", dir.display(), kind.what());
                        let since = start_of(&dir);
                        watchers.push(Watcher::new(dir, kind, since));
                    }
                }
            }
            for w in &mut watchers {
                while let Some(found) = w.poll(platform::is_screen_capture) {
                    let cap = match clip_of(&found, w.kind) {
                        Some(cap) => cap,
                        None if w.retry(found.clone()) => continue,
                        // Never became a readable image: still a download.
                        None if w.kind == Kind::Downloads => file_clip(&found.path),
                        None => {
                            eprintln!("copypasta: can't read screenshot {}", found.path.display());
                            continue;
                        }
                    };
                    let note = match w.kind {
                        Kind::Downloads if !platform::is_screen_capture(&found.path) => "Download",
                        _ => "Screenshot",
                    };
                    let ts = found.at.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
                    if tx.send(Msg::File { cap, note, ts }).is_err() {
                        return;
                    }
                }
            }
            std::thread::sleep(POLL);
        }
    };
    std::thread::Builder::new().name("folders".into()).spawn(run).expect("spawn folders thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let i = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("copypasta-watch-{}-{n}-{i}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn names(w: &mut Watcher, tagged: impl Fn(&Path) -> bool) -> Vec<String> {
        let mut v: Vec<String> =
            std::iter::from_fn(|| w.poll(&tagged)).map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }
    fn tagged(p: &Path) -> bool {
        p.file_name().unwrap().to_string_lossy().starts_with("Screenshot")
    }
    fn pause() {
        std::thread::sleep(Duration::from_millis(20));
    }

    #[test]
    fn finds_new_screenshots_only() {
        let dir = tempdir();
        std::fs::write(dir.join("Screenshot old.png"), b"x").unwrap();
        pause();
        let mut w = Watcher::new(dir.clone(), Kind::Screenshots, SystemTime::now());
        assert!(names(&mut w, tagged).is_empty(), "files from before the watch are left alone");

        pause();
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
        assert_eq!(names(&mut w, |_| true), ["Screenshot empty.png", "notes.png"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finds_every_finished_download() {
        let dir = tempdir();
        std::fs::write(dir.join("old.pdf"), b"x").unwrap();
        pause();
        let since = SystemTime::now();
        let mut w = Watcher::new(dir.clone(), Kind::Downloads, since);
        pause();
        std::fs::write(dir.join("report.pdf"), b"x").unwrap();
        std::fs::write(dir.join("model.stl.crdownload"), b"x").unwrap();
        std::fs::write(dir.join(".DS_Store"), b"x").unwrap();
        std::fs::create_dir(dir.join("unzipped")).unwrap();
        // An old download that was only touched since (as opening it does).
        std::fs::set_permissions(dir.join("old.pdf"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        assert_eq!(names(&mut w, |_| false), ["report.pdf"], "any new file, tagged or not; nothing unfinished");

        // The browser gives a finished download its name, and that is when
        // it arrives, however long ago the download began.
        pause();
        std::fs::rename(dir.join("model.stl.crdownload"), dir.join("model.stl")).unwrap();
        let arrived = w.poll(|_| false).unwrap();
        assert_eq!(arrived.path, dir.join("model.stl"));
        let born = std::fs::metadata(&arrived.path).unwrap().created().unwrap();
        assert!(arrived.at > born && arrived.at > since);

        // Downloaded again over the same name: a different file, so new.
        pause();
        std::fs::write(dir.join("report.pdf.part"), b"xy").unwrap();
        std::fs::rename(dir.join("report.pdf.part"), dir.join("report.pdf")).unwrap();
        assert_eq!(names(&mut w, |_| false), ["report.pdf"]);
        assert!(names(&mut w, |_| false).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retries_only_fresh_files() {
        let dir = tempdir();
        let mut w = Watcher::new(dir.clone(), Kind::Screenshots, UNIX_EPOCH);
        assert!(w.retry(Found { path: dir.join("a.png"), at: SystemTime::now(), len: 1 }));
        assert!(w.poll(|_| true).is_none(), "not before the recheck delay");
        std::thread::sleep(RECHECK + Duration::from_millis(50));
        assert!(w.poll(|_| true).is_some());
        assert!(!w.retry(Found { path: dir.join("b.png"), at: SystemTime::now() - SETTLE, len: 1 }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn downloads_become_images_or_files() {
        let dir = tempdir();
        let found = |name: &str| {
            let path = dir.join(name);
            let len = std::fs::metadata(&path).unwrap().len();
            Found { path, at: SystemTime::now(), len }
        };
        let png = {
            let mut out = std::io::Cursor::new(Vec::new());
            image::RgbImage::from_pixel(6, 4, image::Rgb([9, 9, 9])).write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        std::fs::write(dir.join("diagram.png"), &png).unwrap();
        std::fs::write(dir.join("broken.png"), &png[..png.len() / 2]).unwrap();
        std::fs::write(dir.join("notes.pdf"), b"%PDF").unwrap();

        let image = clip_of(&found("diagram.png"), Kind::Downloads).unwrap();
        assert_eq!((image.kind, image.width, image.height), (ClipKind::Image, Some(6), Some(4)));
        assert_eq!(image.path.as_deref(), dir.join("diagram.png").to_str());

        let file = clip_of(&found("notes.pdf"), Kind::Downloads).unwrap();
        assert_eq!(file.kind, ClipKind::File);
        assert_eq!(file.text.as_deref(), dir.join("notes.pdf").to_str());
        assert_eq!(file.path, file.text);

        // Too big to keep a copy of: listed as a file.
        let big = Found { len: MAX_DOWNLOAD_IMAGE_BYTES + 1, ..found("diagram.png") };
        assert_eq!(clip_of(&big, Kind::Downloads).unwrap().kind, ClipKind::File);
        // Not readable yet: asked for again, in either folder.
        assert!(clip_of(&found("broken.png"), Kind::Downloads).is_none());
        assert!(clip_of(&found("broken.png"), Kind::Screenshots).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
