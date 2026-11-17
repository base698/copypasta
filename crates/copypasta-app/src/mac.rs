//! macOS glue: frontmost app/window, browser URLs, the ⌘V event tap,
//! permission checks, synthesized paste, the screenshot and Downloads
//! folders and the `copypasta panel` request.

use copypasta_core::Context;
use core_foundation::base::{CFRelease, CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::notification_center::{
    CFNotificationCenterAddObserver, CFNotificationCenterGetDarwinNotifyCenter, CFNotificationCenterRef,
    CFNotificationName, CFNotificationSuspensionBehaviorDeliverImmediately,
};
use core_foundation_sys::preferences::{CFPreferencesAppSynchronize, CFPreferencesCopyAppValue};
use objc2_app_kit::NSWorkspace;
use std::ffi::{c_char, c_void, CString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

type AXUIElementRef = *const c_void;
type CFMachPortRef = *mut c_void;
type CGEventRef = *mut c_void;
type TapCallback = extern "C" fn(*mut c_void, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, cb: TapCallback, info: *mut c_void) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetIntegerValueField(ev: CGEventRef, field: u32) -> i64;
    fn CGEventGetFlags(ev: CGEventRef) -> u64;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGEventCreateKeyboardEvent(source: *const c_void, keycode: u16, down: bool) -> CGEventRef;
    fn CGEventSetFlags(ev: CGEventRef, flags: u64);
    fn CGEventPost(tap: u32, ev: CGEventRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: CFMachPortRef, order: isize) -> *mut c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: CFStringRef);
    fn CFRunLoopRun();
    static kCFRunLoopCommonModes: CFStringRef;
}

extern "C" {
    fn getxattr(path: *const c_char, name: *const c_char, value: *mut c_void, size: usize, position: u32, options: i32) -> isize;
}

const KEY_V: i64 = 9;
const FLAG_CONTROL: u64 = 0x40000;
const FLAG_COMMAND: u64 = 0x100000;
const KEY_DOWN: u32 = 10;
const TAP_DISABLED_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_USER: u32 = 0xFFFF_FFFF;

pub const OWN_BUNDLE: &str = "dev.base698.copypasta";

#[derive(Debug, Clone)]
pub struct Frontmost {
    pub name: Option<String>,
    pub bundle_id: Option<String>,
    pub pid: i32,
    /// The app's executable.
    pub exe: Option<PathBuf>,
}

pub fn frontmost() -> Option<Frontmost> {
    let ws = NSWorkspace::sharedWorkspace();
    let app = ws.frontmostApplication()?;
    Some(Frontmost {
        name: app.localizedName().map(|s| s.to_string()),
        bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
        pid: app.processIdentifier(),
        exe: app.executableURL().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string())),
    })
}

fn ax_copy(el: AXUIElementRef, attr: &str) -> Option<CFTypeRef> {
    let name = CFString::new(attr);
    let mut out: CFTypeRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(el, name.as_concrete_TypeRef(), &mut out) };
    (err == 0 && !out.is_null()).then_some(out)
}

/// Title of the focused window of `pid` (needs Accessibility permission).
pub fn window_title(pid: i32) -> Option<String> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let title = ax_copy(app, "AXFocusedWindow").and_then(|win| {
            let t = ax_copy(win, "AXTitle").map(|t| CFString::wrap_under_create_rule(t as CFStringRef).to_string());
            CFRelease(win);
            t
        });
        CFRelease(app as CFTypeRef);
        title.filter(|t| !t.is_empty())
    }
}

const CHROMIUM: &[&str] = &[
    "com.google.Chrome",
    "com.google.Chrome.canary",
    "com.brave.Browser",
    "com.microsoft.edgemac",
    "com.vivaldi.Vivaldi",
    "company.thebrowser.Browser",
    "org.chromium.Chromium",
];
const TERMINALS: &[&str] = &[
    "com.mitchellh.ghostty",
    "com.googlecode.iterm2",
    "com.apple.Terminal",
    "net.kovidgoyal.kitty",
    "com.github.wez.wezterm",
    "org.alacritty",
];

/// The herdr desktop app: a terminal of its own, not a herdr client inside
/// one of `TERMINALS`.
const HERDR_APP: &str = "dev.herdr.app";

/// Apps where a pasted file path should be quoted as a dropped file is.
pub fn is_terminal(bundle: &str) -> bool {
    TERMINALS.contains(&bundle) || bundle == HERDR_APP
}

pub fn is_browser(bundle: &str) -> bool {
    CHROMIUM.contains(&bundle) || bundle == "com.apple.Safari" || bundle == "org.mozilla.firefox"
}

/// (url, title) of the active tab via AppleScript (Chromium family, Safari).
pub fn browser_tab(bundle: &str) -> Option<(String, String)> {
    let script = if CHROMIUM.contains(&bundle) {
        format!("tell application id \"{bundle}\" to tell active tab of front window to return (URL) & linefeed & (title)")
    } else if bundle == "com.apple.Safari" {
        "tell application id \"com.apple.Safari\" to tell current tab of front window to return (URL) & linefeed & (name)".into()
    } else {
        return None;
    };
    let out = run_with_timeout(Command::new("/usr/bin/osascript").arg("-e").arg(script), Duration::from_millis(1500))?;
    let mut lines = out.lines();
    let url = lines.next()?.trim().to_string();
    let title = lines.collect::<Vec<_>>().join("\n");
    url.starts_with("http").then_some((url, title))
}

pub fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<String> {
    use std::io::Read;
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null()).spawn().ok()?;
    // Read while it runs: a child with more output than the pipe holds
    // would otherwise wait on us forever, and so time out.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        stdout.read_to_string(&mut s).ok().map(|_| s)
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().ok()??;
                return status.success().then_some(out);
            }
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Strip " - Google Chrome", " — Mozilla Firefox" and similar from titles.
fn clean_title(t: &str) -> String {
    let mut t = t.to_string();
    for suffix in [" - Google Chrome", " - Brave", " — Mozilla Firefox", " - Mozilla Firefox", " - Microsoft Edge", " - Chromium"] {
        if let Some(i) = t.find(suffix) {
            t.truncate(i);
        }
    }
    t
}

/// Everything we can learn about the frontmost window right now.
pub fn current_context(page_url_hint: Option<&str>) -> Context {
    let Some(app) = frontmost() else { return Context::default() };
    let bundle = app.bundle_id.clone().unwrap_or_default();
    let mut ctx = Context {
        app_name: app.name.clone(),
        bundle_id: app.bundle_id.clone(),
        window_title: window_title(app.pid).map(|t| clean_title(&t)),
        ..Default::default()
    };
    if is_browser(&bundle) {
        if let Some((url, title)) = browser_tab(&bundle) {
            ctx.url = Some(url);
            ctx.page_title = Some(title).filter(|t| !t.is_empty());
        } else if let Some(u) = page_url_hint {
            ctx.url = Some(u.to_string());
        }
    }
    if TERMINALS.contains(&bundle.as_str()) {
        ctx.herdr = crate::herdr::focused_pane();
    } else if bundle == HERDR_APP {
        ctx.herdr = app.exe.as_deref().and_then(crate::herdr::app_focused_pane);
    }
    ctx.enrich()
}

// ---- permissions -------------------------------------------------------

pub fn accessibility_trusted(prompt: bool) -> bool {
    unsafe {
        if !prompt {
            return AXIsProcessTrusted();
        }
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let dict = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
        AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef() as *const c_void)
    }
}

pub fn input_monitoring_granted(prompt: bool) -> bool {
    unsafe {
        if CGPreflightListenEventAccess() {
            return true;
        }
        prompt && CGRequestListenEventAccess()
    }
}

// ---- ⌘V event tap ------------------------------------------------------

static TAP_TX: OnceLock<Sender<crate::capture::Msg>> = OnceLock::new();
static mut TAP_PORT: CFMachPortRef = std::ptr::null_mut();

extern "C" fn tap_cb(_proxy: *mut c_void, ty: u32, ev: CGEventRef, _info: *mut c_void) -> CGEventRef {
    unsafe {
        if ty == TAP_DISABLED_TIMEOUT || ty == TAP_DISABLED_USER {
            let port = TAP_PORT;
            if !port.is_null() {
                CGEventTapEnable(port, true);
            }
            return ev;
        }
        if ty == KEY_DOWN && CGEventGetIntegerValueField(ev, 9) == KEY_V {
            let flags = CGEventGetFlags(ev);
            // ⌘V, ⇧⌘V, ⌥⇧⌘V — but not ⌃⌘V, which opens our own panel.
            if flags & FLAG_COMMAND != 0 && flags & FLAG_CONTROL == 0 {
                if let Some(tx) = TAP_TX.get() {
                    let _ = tx.send(crate::capture::Msg::Paste { ts: copypasta_core::now_ms() });
                }
            }
        }
    }
    ev
}

/// Start a listen-only key tap on its own thread. Returns false when the
/// Input Monitoring permission is missing.
pub fn start_paste_tap(tx: Sender<crate::capture::Msg>) -> bool {
    let _ = TAP_TX.set(tx);
    let (ok_tx, ok_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("paste-tap".into())
        .spawn(move || unsafe {
            // kCGSessionEventTap = 1, head insert = 0, listen-only = 1
            let port = CGEventTapCreate(1, 0, 1, 1 << KEY_DOWN, tap_cb, std::ptr::null_mut());
            if port.is_null() {
                let _ = ok_tx.send(false);
                return;
            }
            TAP_PORT = port;
            let src = CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
            CFRunLoopAddSource(CFRunLoopGetCurrent(), src, kCFRunLoopCommonModes);
            CGEventTapEnable(port, true);
            let _ = ok_tx.send(true);
            CFRunLoopRun();
        })
        .ok();
    ok_rx.recv_timeout(Duration::from_secs(2)).unwrap_or(false)
}

/// Send ⌘V to whatever app is frontmost (needs Accessibility).
pub fn synthesize_paste() {
    unsafe {
        for down in [true, false] {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), KEY_V as u16, down);
            if ev.is_null() {
                return;
            }
            CGEventSetFlags(ev, FLAG_COMMAND);
            CGEventPost(0, ev);
            CFRelease(ev as CFTypeRef);
        }
    }
}

// ---- panel window ------------------------------------------------------

/// Mark the panel as a window that may share a Space with another app's
/// full-screen window, so it can open over one. This only takes effect for
/// a window created while the app is already an accessory (see `main`).
/// Call on the main thread.
pub fn show_over_full_screen(win: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
    let Ok(ptr) = win.ns_window() else { return };
    let window: &NSWindow = unsafe { &*ptr.cast() };
    window.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary);
}

// ---- screenshots and downloads -----------------------------------------

/// The user's Downloads folder.
pub fn downloads_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Downloads"))
}

/// The folder macOS saves screenshots in (⇧⌘5 → Options → Save to): the
/// Desktop unless changed.
pub fn screenshot_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let domain = CFString::from_static_string("com.apple.screencapture");
    let key = CFString::from_static_string("location");
    let location = unsafe {
        CFPreferencesAppSynchronize(domain.as_concrete_TypeRef());
        let value = CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), domain.as_concrete_TypeRef());
        if value.is_null() {
            None
        } else {
            CFType::wrap_under_create_rule(value).downcast_into::<CFString>().map(|s| s.to_string())
        }
    };
    Some(match location.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        None => home.join("Desktop"),
        Some("~") => home,
        Some(l) => match l.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(l),
        },
    })
}

/// True for files macOS wrote as screenshots: it tags them, whatever the
/// language or the name they are saved under.
pub fn is_screen_capture(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else { return false };
    let tag = c"com.apple.metadata:kMDItemIsScreenCapture";
    unsafe { getxattr(path.as_ptr(), tag.as_ptr(), std::ptr::null_mut(), 0, 0, 0) >= 0 }
}

// ---- `copypasta panel` -------------------------------------------------

static PANEL_REQUEST: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

extern "C" fn panel_requested(
    _center: CFNotificationCenterRef,
    _observer: *mut c_void,
    _name: CFNotificationName,
    _object: *const c_void,
    _info: CFDictionaryRef,
) {
    if let Some(f) = PANEL_REQUEST.get() {
        f();
    }
}

/// Run `f` whenever `copypasta panel` asks for the panel. Call once, on the
/// main thread, whose run loop then delivers the requests.
pub fn on_panel_request(f: impl Fn() + Send + Sync + 'static) {
    if PANEL_REQUEST.set(Box::new(f)).is_err() {
        return;
    }
    let name = CFString::new(copypasta_core::PANEL_NOTIFICATION);
    unsafe {
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDarwinNotifyCenter(),
            std::ptr::null(),
            panel_requested,
            name.as_concrete_TypeRef(),
            std::ptr::null(),
            CFNotificationSuspensionBehaviorDeliverImmediately,
        );
    }
}
