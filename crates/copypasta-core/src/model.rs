use crate::context::Context;
use serde::{Deserialize, Serialize};

/// What the clipboard held, normalized and ready to store.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub kind: ClipKind,
    /// Plain text; for images, the source URL when known.
    pub text: Option<String>,
    /// PNG bytes for image clips.
    pub png: Option<Vec<u8>>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub types: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    #[default]
    Text,
    Url,
    Image,
    File,
}

impl ClipKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ClipKind::Text => "text",
            ClipKind::Url => "url",
            ClipKind::Image => "image",
            ClipKind::File => "file",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "url" => ClipKind::Url,
            "image" => ClipKind::Image,
            "file" => ClipKind::File,
            _ => ClipKind::Text,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Clip {
    pub id: i64,
    pub hash: String,
    pub kind: ClipKind,
    pub text: Option<String>,
    pub preview: String,
    pub blob: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: i64,
    pub secret: bool,
    pub first_seen: i64,
    pub last_seen: i64,
    pub copies: i64,
    pub pastes: i64,
    pub pinned: bool,
}

/// A clip plus the places it was copied from / pasted into, for list views.
#[derive(Debug, Clone, Serialize)]
pub struct ClipRow {
    #[serde(flatten)]
    pub clip: Clip,
    pub sources: Vec<Place>,
    pub destinations: Vec<Place>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Place {
    pub key: String,
    pub label: String,
    pub kind: String,
    pub last_ts: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Copy,
    Paste,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Copy => "copy",
            EventKind::Paste => "paste",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub id: i64,
    pub ts: i64,
    pub kind: EventKind,
    pub clip_id: i64,
    pub ctx_key: String,
    pub label: String,
    pub ctx: Context,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
