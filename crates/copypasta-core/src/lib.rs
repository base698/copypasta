pub mod api;
pub mod context;
pub mod db;
pub mod graph;
pub mod groups;
pub mod imaging;
pub mod lineage;
pub mod model;
pub mod paths;
pub mod secrets;
#[cfg(target_os = "macos")]
pub mod pasteboard;

/// Darwin notification that asks the running app to open or close its panel
/// (`copypasta panel`), for when a key can't reach the global shortcut.
pub const PANEL_NOTIFICATION: &str = "dev.base698.copypasta.panel";

pub use context::Context;
pub use db::Store;
pub use model::*;
