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
mod shots;

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
    // Whoever is in front now is where a paste from the panel will go.
    #[cfg(target_os = "macos")]
    if let Some(front) = mac::frontmost().and_then(|f| f.bundle_id).filter(|b| b != mac::OWN_BUNDLE) {
        if let Some(state) = app.try_state::<AppState>() {
            *state.shared.target_app.lock().unwrap() = Some(front);
        }
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
    #[allow(unused_mut)]
    let mut app = tauri::Builder::default()
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
            if let Some(win) = app.get_webview_window("main") {
                mac::show_over_full_screen(&win);
            }

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
            // `copypasta panel` does what the shortcut does, for apps whose
            // own keys are easier to bind than a global one.
            #[cfg(target_os = "macos")]
            {
                let handle = app.handle().clone();
                mac::on_panel_request(move || toggle_panel(&handle));
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
        .build(tauri::generate_context!())
        .expect("error while starting copypasta");
    // Become a menu-bar-less accessory app before launch, not in `setup`:
    // the panel window is created ahead of `setup`, and a window created
    // while the app is still a regular one can never be shown over another
    // app's full-screen Space, whatever is set on it later.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    app.run(|_, _| {});
}
