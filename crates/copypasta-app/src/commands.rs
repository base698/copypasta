use crate::AppState;
use base64::Engine;
use copypasta_core::{graph, imaging, ClipKind, ClipRow};
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::State;

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub fn search(state: State<AppState>, query: String, limit: Option<usize>) -> Res<Vec<ClipRow>> {
    state.store.lock().unwrap().search(&query, limit.unwrap_or(200)).map_err(err)
}

#[tauri::command]
pub fn graph(state: State<AppState>, id: i64) -> Res<graph::Graph> {
    graph::build(&state.store.lock().unwrap(), id).map_err(err)
}

/// PNG as a data URL; `max` limits the long side for thumbnails.
#[tauri::command]
pub fn image(state: State<AppState>, id: i64, max: Option<u32>) -> Res<Option<String>> {
    let store = state.store.lock().unwrap();
    let Some(clip) = store.clip(id).map_err(err)? else { return Ok(None) };
    let Some(png) = store.read_blob(&clip) else { return Ok(None) };
    let png = match max {
        Some(m) => imaging::thumbnail(&png, m).map_err(err)?,
        None => png,
    };
    Ok(Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))))
}

/// Put a clip back on the clipboard; with `paste`, also paste it into the
/// app that was in front before the panel opened.
#[tauri::command]
pub fn restore(app: tauri::AppHandle, state: State<AppState>, id: i64, paste: bool) -> Res<()> {
    let store = state.store.lock().unwrap();
    let clip = store.clip(id).map_err(err)?.ok_or("clip not found")?;
    if clip.secret {
        return Err("secret clips are not stored and cannot be restored".into());
    }
    *state.shared.pending_restore.lock().unwrap() = Some(id);
    #[cfg(target_os = "macos")]
    {
        use copypasta_core::pasteboard;
        let ok = match clip.kind {
            ClipKind::Image => match store.read_blob(&clip) {
                Some(png) => pasteboard::write_png(&png, clip.text.as_deref()),
                None => false,
            },
            _ => pasteboard::write_text(clip.text.as_deref().unwrap_or("")),
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

#[tauri::command]
pub fn delete(state: State<AppState>, id: i64) -> Res<()> {
    state.store.lock().unwrap().delete_clip(id).map_err(err)
}

#[tauri::command]
pub fn pin(state: State<AppState>, id: i64, pinned: bool) -> Res<()> {
    state.store.lock().unwrap().set_pinned(id, pinned).map_err(err)
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
