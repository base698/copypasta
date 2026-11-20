use crate::AppState;
use copypasta_core::{Clip, ClipKind};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::State;

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Every store operation the panel needs (see copypasta_core::api).
#[tauri::command]
pub fn api(state: State<AppState>, cmd: String, args: Option<serde_json::Value>) -> Res<serde_json::Value> {
    let mut store = state.store.lock().unwrap();
    copypasta_core::api::call(&mut store, &cmd, &args.unwrap_or(serde_json::Value::Null)).map_err(err)
}

/// Put clips back on the clipboard; with `paste`, also paste them into the
/// app that was in front before the panel opened. With `as_path`, a clip
/// that has a file (a screenshot, any image, copied files) goes back as
/// that file's path instead of its content.
///
/// Several clips go back together: images and files as the files
/// themselves (as Finder copies them), or as their paths in a terminal or
/// with `as_path`; anything with text as that text, one per line.
#[tauri::command]
pub fn restore(app: tauri::AppHandle, state: State<AppState>, ids: Vec<i64>, paste: bool, as_path: Option<bool>) -> Res<()> {
    let store = state.store.lock().unwrap();
    let mut clips = Vec::with_capacity(ids.len());
    for &id in &ids {
        let clip = store.clip(id).map_err(err)?.ok_or("clip not found")?;
        if clip.secret {
            return Err("secret clips are not stored and cannot be restored".into());
        }
        clips.push(clip);
    }
    if clips.is_empty() {
        return Err("nothing selected".into());
    }
    let terminal = state.shared.target_app.lock().unwrap().as_deref().is_some_and(is_terminal);
    let as_path = as_path.unwrap_or(false);
    let files: Vec<Vec<PathBuf>> = clips.iter().map(|c| store.files(c)).collect();
    *state.shared.pending_restore.lock().unwrap() = Some(ids);
    #[cfg(target_os = "macos")]
    {
        use copypasta_core::pasteboard;
        let ok = if let [clip] = clips.as_slice() {
            match (&files[0], clip.kind) {
                (f, _) if as_path && !f.is_empty() => pasteboard::write_text(&paths_text(f, terminal)),
                (_, ClipKind::Image) => match store.read_blob(clip) {
                    Some(png) => pasteboard::write_png(&png, clip.text.as_deref()),
                    None => false,
                },
                _ => pasteboard::write_text(clip.text.as_deref().unwrap_or("")),
            }
        } else if !as_path && !terminal && files.iter().all(|f| !f.is_empty()) {
            pasteboard::write_files(&files.concat())
        } else {
            pasteboard::write_text(&joined_text(&clips, &files, terminal))
        };
        if !ok {
            return Err("could not write to the clipboard".into());
        }
    }
    drop(store);
    crate::hide_panel(&app);
    if paste {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(180));
            #[cfg(target_os = "macos")]
            crate::mac::synthesize_paste();
        });
    }
    Ok(())
}

/// Paths as text to paste. A terminal gets what dropping the files on it
/// would type (backslash-quoted, space-separated), which is also the form
/// agents such as Claude Code recognize as an attached image; other apps
/// get the plain paths, one per line.
fn paths_text(files: &[PathBuf], terminal: bool) -> String {
    let paths = files.iter().map(|p| p.to_string_lossy());
    if terminal {
        paths.map(|p| shell_quote(&p)).collect::<Vec<_>>().join(" ")
    } else {
        paths.collect::<Vec<_>>().join("\n")
    }
}

/// Several clips as one text: each clip's file paths where it has files,
/// else its text. Only paths: on one line in a terminal, as dropping the
/// files would type them; otherwise a line each.
fn joined_text(clips: &[Clip], files: &[Vec<PathBuf>], terminal: bool) -> String {
    let all_files = files.iter().all(|f| !f.is_empty());
    let parts: Vec<String> = clips
        .iter()
        .zip(files)
        .map(|(c, f)| if f.is_empty() { c.text.clone().unwrap_or_default() } else { paths_text(f, terminal) })
        .collect();
    parts.join(if all_files && terminal { " " } else { "\n" })
}

#[cfg(target_os = "macos")]
fn is_terminal(bundle: &str) -> bool {
    crate::mac::is_terminal(bundle)
}
#[cfg(not(target_os = "macos"))]
fn is_terminal(_: &str) -> bool {
    false
}

fn shell_quote(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    for c in path.chars() {
        // Non-ASCII (macOS puts U+202F in screenshot names) needs no quoting.
        if c.is_ascii() && !c.is_ascii_alphanumeric() && !"/._-+=:,@%".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[derive(Serialize)]
pub struct Status {
    paste_tracking: bool,
    accessibility: bool,
    stats: std::collections::HashMap<&'static str, i64>,
}

#[tauri::command]
pub fn status(state: State<AppState>) -> Res<Status> {
    Ok(Status {
        paste_tracking: state.shared.paste_tracking.load(Ordering::Relaxed),
        #[cfg(target_os = "macos")]
        accessibility: crate::mac::accessibility_trusted(false),
        #[cfg(not(target_os = "macos"))]
        accessibility: true,
        stats: state.store.lock().unwrap().stats().map_err(err)?,
    })
}

#[tauri::command]
pub fn hide(app: tauri::AppHandle) {
    crate::hide_panel(&app);
}

#[tauri::command]
pub fn request_permissions() {
    #[cfg(target_os = "macos")]
    {
        crate::mac::accessibility_trusted(true);
        crate::mac::input_monitoring_granted(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_quoted_for_terminals_only() {
        let files = [PathBuf::from("/Users/me/Documents/Screenshot 2026-01-02 at 3.04.05\u{202f}PM.png"), PathBuf::from("/tmp/it's (1).png")];
        assert_eq!(
            paths_text(&files, true),
            "/Users/me/Documents/Screenshot\\ 2026-01-02\\ at\\ 3.04.05\u{202f}PM.png /tmp/it\\'s\\ \\(1\\).png"
        );
        assert_eq!(paths_text(&files, false), "/Users/me/Documents/Screenshot 2026-01-02 at 3.04.05\u{202f}PM.png\n/tmp/it's (1).png");
    }

    #[test]
    fn several_clips_join_as_paths_or_lines() {
        let text = |t: &str| Clip { text: Some(t.into()), ..Default::default() };
        let shots = [text("a"), text("b")];
        let files = [vec![PathBuf::from("/d/a b.png")], vec![PathBuf::from("/d/c.png")]];
        assert_eq!(joined_text(&shots, &files, true), "/d/a\\ b.png /d/c.png");
        assert_eq!(joined_text(&shots, &files, false), "/d/a b.png\n/d/c.png");
        let mixed = [text("fix the bug"), text("x")];
        let files = [vec![], vec![PathBuf::from("/d/c.png")]];
        assert_eq!(joined_text(&mixed, &files, true), "fix the bug\n/d/c.png");
    }
}
