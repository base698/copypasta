// Copypasta: clipboard history with copy/paste lineage.
// Runs as a menu-bar-less background app; ⌃⌘V toggles the panel.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod platform {
    pub use crate::mac::*;
    pub use copypasta_core::pasteboard::{change_count, read};
}

mod capture;
mod commands;
mod herdr;

use capture::Shared;
use copypasta_core::Store;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

pub struct AppState {
    pub store: Mutex<Store>,
    pub shared: Arc<Shared>,
}

fn toggle_panel(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window("main") else { return };
    if win.is_visible().unwrap_or(false) && win.is_focused().unwrap_or(false) {
        hide_panel(app);
        return;
    }
    let _ = win.center();
    let _ = win.show();
    let _ = win.set_focus();
    let _ = win.emit("panel-shown", ());
}

pub fn hide_panel(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
    // Hand focus back to the app that was in front before the panel.
    #[cfg(target_os = "macos")]
    let _ = app.hide();
}

fn main() {
    let hotkey = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SUPER), Code::KeyV);
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, sc, ev| {
                    if sc == &hotkey && ev.state() == ShortcutState::Pressed {
                        toggle_panel(app);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let shared = Arc::new(Shared::default());
            let handle = app.handle().clone();
            capture::spawn(
                Store::open()?,
                shared.clone(),
                Box::new(move || {
                    let _ = handle.emit("clips-changed", ());
                }),
            );
            app.manage(AppState { store: Mutex::new(Store::open()?), shared });

            #[cfg(target_os = "macos")]
            if !mac::accessibility_trusted(false) {
                mac::accessibility_trusted(true);
            }
            if let Err(e) = app.global_shortcut().register(hotkey) {
                eprintln!("copypasta: could not register ⌃⌘V: {e}");
            }
            if std::env::args().any(|a| a == "--show") {
                toggle_panel(app.handle());
            }
            Ok(())
        })
        .on_window_event(|win, ev| {
            if let WindowEvent::Focused(false) = ev {
                if std::env::var("COPYPASTA_PIN_PANEL").is_err() {
                    let _ = win.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::api,
            commands::restore,
            commands::status,
            commands::hide,
            commands::request_permissions,
        ])
        .run(tauri::generate_context!())
        .expect("error while running copypasta");
}
