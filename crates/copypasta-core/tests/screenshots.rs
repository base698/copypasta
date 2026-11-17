//! Clips read from files (saved screenshots, downloads): recorded at the
//! time they were taken or arrived, with the file they came from, and
//! pasteable as a path.

use copypasta_core::{imaging, Captured, ClipKind, Context, Store};
use std::path::{Path, PathBuf};

fn tempdir() -> PathBuf {
    // Tests run in parallel and the clock is coarse: number the folders too.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let i = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let d = std::env::temp_dir().join(format!("copypasta-shots-{}-{n}-{i}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Write a small PNG and return what the capture loop builds from it.
fn screenshot(file: &Path, shade: u8) -> Captured {
    let img = image::RgbImage::from_fn(40, 30, |x, y| image::Rgb([shade, (x * 6) as u8, (y * 8) as u8]));
    img.save(file).unwrap();
    let n = imaging::to_png(&std::fs::read(file).unwrap()).unwrap();
    Captured {
        kind: ClipKind::Image,
        png: Some(n.png),
        width: Some(n.width),
        height: Some(n.height),
        path: Some(file.to_string_lossy().into_owned()),
        ..Default::default()
    }
}

fn taken() -> Context {
    Context { note: Some("Screenshot".into()), ..Default::default() }
}

#[test]
fn saved_screenshots_sit_among_clips_by_time_and_paste_as_paths() {
    let dir = tempdir();
    let mut s = Store::open_at(&dir).unwrap();
    let text = |t: &str| Captured { kind: ClipKind::Text, text: Some(t.into()), ..Default::default() };
    let app = Context { app_name: Some("Notes".into()), ..Default::default() };

    let (before, _) = s.record_copy(&text("copied before the screenshot"), &app, 1_000).unwrap();
    let (after, _) = s.record_copy(&text("copied after the screenshot"), &app, 3_000).unwrap();
    // The file shows up late (macOS holds it while the thumbnail floats),
    // but it is recorded at the time it was taken.
    let file = dir.join("Screenshot 2026-01-02 at 3.04.05 PM.png");
    let (shot, _) = s.record_copy(&screenshot(&file, 10), &taken(), 2_000).unwrap();

    let order: Vec<i64> = s.search("", 10).unwrap().iter().map(|r| r.clip.id).collect();
    assert_eq!(order, [after, shot, before]);

    let clip = s.clip(shot).unwrap().unwrap();
    assert_eq!(clip.path.as_deref(), Some(file.to_string_lossy().as_ref()));
    assert_eq!(clip.preview, "Image 40×30 · Screenshot 2026-01-02 at 3.04.05 PM.png");
    assert_eq!(s.search("screenshot 3.04", 10).unwrap().len(), 1, "found by file name");
    assert_eq!(s.newest_file_in(&dir).unwrap(), Some(2_000));
    assert_eq!(s.newest_file_in(&dir.join("elsewhere")).unwrap(), None, "each folder has its own newest file");

    // The path to paste is the screenshot itself while it exists, then our copy.
    assert_eq!(s.files(&clip), [file.clone()]);
    std::fs::remove_file(&file).unwrap();
    assert_eq!(s.files(&clip), [s.blob_path(&clip).unwrap()]);
    assert!(s.files(&s.clip(before).unwrap().unwrap()).is_empty(), "text has no file");
}

#[test]
fn seeing_the_same_file_again_changes_nothing() {
    let dir = tempdir();
    let mut s = Store::open_at(&dir).unwrap();
    let file = dir.join("Screenshot.png");
    let cap = screenshot(&file, 200);
    let (shot, _) = s.record_copy(&cap, &taken(), 5_000).unwrap();
    // Pasted later, then the app restarts and finds the file again.
    s.record_paste(shot, &Context { app_name: Some("Herdr".into()), ..Default::default() }, 9_000).unwrap();
    s.record_copy(&cap, &taken(), 5_000).unwrap();

    let clip = s.clip(shot).unwrap().unwrap();
    assert_eq!((clip.copies, clip.pastes), (1, 1));
    assert_eq!(clip.last_seen, 9_000, "an old screenshot time never moves a clip back");

    // An image copied to the clipboard has no file of its own, only our copy.
    let mut copied = screenshot(&dir.join("other.png"), 90);
    copied.path = None;
    let (id, _) = s.record_copy(&copied, &taken(), 9_500).unwrap();
    let c = s.clip(id).unwrap().unwrap();
    assert_eq!(c.path, None);
    assert_eq!(s.newest_file_in(&dir).unwrap(), Some(5_000));
    assert_eq!(s.files(&c), [s.blob_path(&c).unwrap()]);
}

#[test]
fn downloads_are_listed_by_path() {
    let dir = tempdir();
    let mut s = Store::open_at(&dir).unwrap();
    let file = dir.join("quarterly report.pdf");
    let path = file.to_string_lossy().into_owned();
    let download = Captured { kind: ClipKind::File, text: Some(path.clone()), path: Some(path.clone()), ..Default::default() };
    let arrived = Context { note: Some("Download".into()), ..Default::default() };
    let (id, _) = s.record_copy(&download, &arrived, 1_000).unwrap();

    let row = &s.search("quarterly", 10).unwrap()[0];
    assert_eq!((row.clip.id, row.clip.kind, row.clip.preview.as_str()), (id, ClipKind::File, path.as_str()));
    assert_eq!(row.sources[0].label, "Download");
    assert_eq!(s.files(&row.clip), [file.clone()]);
    assert_eq!(s.newest_file_in(&dir).unwrap(), Some(1_000));

    // The same file copied in Finder later is the same clip, with both sources.
    let copied = Captured { kind: ClipKind::File, text: Some(path), ..Default::default() };
    let (again, _) = s.record_copy(&copied, &Context { app_name: Some("Finder".into()), ..Default::default() }, 2_000).unwrap();
    assert_eq!(again, id);
    let clip = s.clip(id).unwrap().unwrap();
    assert_eq!((clip.copies, clip.last_seen, clip.path.as_deref()), (2, 2_000, file.to_str()));
}
