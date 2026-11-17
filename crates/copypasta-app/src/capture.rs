//! The capture loop: watches the clipboard, records copies with the
//! frontmost window's context, and records pastes reported by the key tap.

use crate::platform;
use crate::shots::{Shot, Watcher};
use copypasta_core::{imaging, now_ms, Captured, ClipKind, Context, Store};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const RETENTION_MS: i64 = 30 * 86_400_000;
pub const MAX_BYTES: i64 = 2 * 1024 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(150);
/// How far back saved screenshots are picked up the first time.
const SCREENSHOT_BACKFILL_MS: i64 = 86_400_000;
/// How often to check whether the screenshot folder setting changed.
const SCREENSHOT_DIR_CHECK: Duration = Duration::from_secs(60);

pub enum Msg {
    Paste { ts: i64 },
}

#[derive(Default)]
pub struct Shared {
    /// Clip the panel just put back on the clipboard.
    pub pending_restore: Mutex<Option<i64>>,
    pub paste_tracking: AtomicBool,
    /// Bundle id of the app that was in front when the panel opened: where
    /// a paste from the panel goes.
    pub target_app: Mutex<Option<String>>,
}

pub struct Capture {
    store: Store,
    shared: Arc<Shared>,
    last_change: isize,
    current: Option<i64>,
    on_change: Box<dyn Fn() + Send>,
    /// Watches the folder macOS saves screenshots in.
    shots: Option<Watcher>,
    shots_checked: Option<Instant>,
}

impl Capture {
    pub fn new(mut store: Store, shared: Arc<Shared>, on_change: Box<dyn Fn() + Send>) -> Self {
        match store.repair() {
            Ok(n) if n > 0 => eprintln!("copypasta: repaired workspace on {n} events"),
            Err(e) => eprintln!("copypasta: repair failed: {e:#}"),
            _ => {}
        }
        let mut c = Capture {
            store,
            shared,
            last_change: platform::change_count(),
            current: None,
            on_change,
            shots: None,
            shots_checked: None,
        };
        // Whatever is on the clipboard at startup: remember it without a copy event.
        if let Some(cap) = platform::read().to_captured() {
            c.current = c.store.upsert_clip(&cap, now_ms()).ok();
        }
        c
    }

    pub fn run(mut self, rx: Receiver<Msg>) {
        let mut last_gc = Instant::now() - Duration::from_secs(3600);
        loop {
            match rx.recv_timeout(POLL) {
                Ok(Msg::Paste { ts }) => {
                    self.poll();
                    self.on_paste(ts);
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.poll();
                    self.poll_screenshots();
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if last_gc.elapsed() > Duration::from_secs(3600) {
                last_gc = Instant::now();
                match self.store.gc(now_ms(), RETENTION_MS, MAX_BYTES) {
                    Ok(n) if n > 0 => eprintln!("copypasta: gc removed {n} clips"),
                    Err(e) => eprintln!("copypasta: gc failed: {e:#}"),
                    _ => {}
                }
            }
        }
    }

    fn poll(&mut self) {
        let cc = platform::change_count();
        if cc == self.last_change {
            return;
        }
        self.last_change = cc;
        let snap = platform::read();
        if snap.is_ours() {
            // Put back by the panel (we know the id) or by the TUI (look it up).
            let pending = self.shared.pending_restore.lock().unwrap().take();
            let found = pending.or_else(|| {
                let cap = snap.content()?;
                self.store.clip_id_by_hash(&copypasta_core::db::clip_hash(&cap)).ok().flatten()
            });
            if found.is_some() {
                self.current = found;
            }
            return;
        }
        let Some(cap) = snap.to_captured() else {
            // Concealed (password manager) or unsupported: pastes until the
            // next copy are of something we didn't record.
            self.current = None;
            return;
        };
        let mut ctx = platform::current_context(snap.source_url.as_deref());
        if is_own(&ctx) {
            return;
        }
        if copypasta_core::secrets::is_password_manager(ctx.bundle_id.as_deref()) {
            // Not recorded; and a paste now is of something we don't know.
            self.current = None;
            return;
        }
        if cap.kind == copypasta_core::ClipKind::Image && looks_like_screenshot(&snap.types) {
            ctx = Context {
                note: Some("Screenshot".into()),
                app_name: ctx.app_name,
                bundle_id: ctx.bundle_id,
                window_title: ctx.window_title,
                ..Default::default()
            };
        }
        match self.store.record_copy(&cap, &ctx, now_ms()) {
            Ok((clip, _)) => {
                self.current = Some(clip);
                (self.on_change)();
            }
            Err(e) => eprintln!("copypasta: record copy failed: {e:#}"),
        }
    }

    /// Record screenshots saved as files, one per tick. They are not on the
    /// clipboard, so `current` (what a ⌘V pastes) stays as it is.
    fn poll_screenshots(&mut self) {
        if self.shots_checked.map(|t| t.elapsed() >= SCREENSHOT_DIR_CHECK).unwrap_or(true) {
            self.shots_checked = Some(Instant::now());
            let dir = platform::screenshot_dir();
            if dir.as_deref() != self.shots.as_ref().map(Watcher::dir) {
                if let Some(d) = &dir {
                    eprintln!("copypasta: watching {} for screenshots", d.display());
                }
                // Not older than the newest one already recorded, or a day.
                let newest = self.store.newest_file_clip().ok().flatten().map(|ms| ms + 1).unwrap_or(0);
                let since = newest.max(now_ms() - SCREENSHOT_BACKFILL_MS).max(0) as u64;
                self.shots = dir.map(|d| Watcher::new(d, UNIX_EPOCH + Duration::from_millis(since)));
            }
        }
        let Some(watcher) = &mut self.shots else { return };
        let Some(shot) = watcher.poll(platform::is_screen_capture) else { return };
        let read = std::fs::read(&shot.path).map_err(anyhow::Error::from).and_then(|bytes| imaging::to_png(&bytes));
        let img = match read {
            Ok(img) => img,
            Err(e) => {
                let path = shot.path.clone();
                if !watcher.retry(shot) {
                    eprintln!("copypasta: can't read screenshot {}: {e:#}", path.display());
                }
                return;
            }
        };
        let Shot { path, taken } = shot;
        let cap = Captured {
            kind: ClipKind::Image,
            png: Some(img.png),
            width: Some(img.width),
            height: Some(img.height),
            path: Some(path.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let ctx = Context { note: Some("Screenshot".into()), ..Default::default() };
        let ts = taken.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_else(|_| now_ms());
        match self.store.record_copy(&cap, &ctx, ts) {
            Ok(_) => (self.on_change)(),
            Err(e) => eprintln!("copypasta: record screenshot failed: {e:#}"),
        }
    }

    fn on_paste(&mut self, ts: i64) {
        let Some(clip) = self.current else { return };
        let ctx = platform::current_context(None);
        if is_own(&ctx) {
            return;
        }
        match self.store.record_paste(clip, &ctx, ts) {
            Ok(_) => (self.on_change)(),
            Err(e) => eprintln!("copypasta: record paste failed: {e:#}"),
        }
    }
}

fn is_own(ctx: &Context) -> bool {
    ctx.bundle_id.as_deref() == Some(platform::OWN_BUNDLE) || ctx.app_name.as_deref() == Some("copypasta-app")
}

/// macOS region screenshots (⌃⇧⌘4) put bare image data on the clipboard,
/// unlike browser/app image copies which carry HTML or a source URL.
fn looks_like_screenshot(types: &[String]) -> bool {
    !types.iter().any(|t| {
        t.contains("html") || t.contains("url") || t.contains("chromium") || t.contains("rtf") || t.contains("text")
    })
}

/// Start capture on a background thread, plus the paste tap. Without the
/// Input Monitoring permission the tap is retried until it is granted.
pub fn spawn(store: Store, shared: Arc<Shared>, on_change: Box<dyn Fn() + Send>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let tracking = platform::start_paste_tap(tx.clone());
    shared.paste_tracking.store(tracking, Ordering::Relaxed);
    eprintln!("copypasta: started; paste tracking {}", if tracking { "on" } else { "off" });
    if !tracking {
        eprintln!("copypasta: paste tracking off until Input Monitoring is granted to Copypasta");
        platform::input_monitoring_granted(true);
        let shared = shared.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(10));
            if platform::input_monitoring_granted(false) && platform::start_paste_tap(tx.clone()) {
                shared.paste_tracking.store(true, Ordering::Relaxed);
                eprintln!("copypasta: paste tracking on");
                return;
            }
        });
    }
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || Capture::new(store, shared, on_change).run(rx))
        .expect("spawn capture thread");
}
