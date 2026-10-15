use crate::AppState;
use copypasta_core::ClipKind;
use serde::Serialize;
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
