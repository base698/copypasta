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
    /// For a clip read from a file rather than the clipboard (a saved
    /// screenshot, a download): that file.
    pub path: Option<String>,
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

#[derive(Debug, Clone, Default, Serialize)]
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
    /// The file the clip was read from (a saved screenshot, a download).
    pub path: Option<String>,
}

/// A clip plus the places it was copied from / pasted into, for list views.
#[derive(Debug, Clone, Serialize)]
pub struct ClipRow {
    #[serde(flatten)]
    pub clip: Clip,
    pub sources: Vec<Place>,
    pub destinations: Vec<Place>,
    pub group: Option<GroupRef>,
    /// Listed in a group it doesn't belong to because it was pasted into
    /// that group's herdr workspace.
    pub incoming: bool,
    /// herdr workspaces this clip was copied from or pasted into.
    pub workspaces: Vec<String>,
    /// An earlier clip this one is an edited version of (near-identical text).
    pub edit_of: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GroupRef {
    pub id: i64,
    pub name: String,
    pub manual: bool,
}

/// A group: a herdr workspace (automatic) or one the user made.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    /// The herdr workspace label for automatic groups.
    pub workspace: Option<String>,
    pub created: i64,
}

/// One entry of the grouped list; `id` None is "Ungrouped".
#[derive(Debug, Clone, Serialize)]
pub struct GroupSummary {
    pub id: Option<i64>,
    pub name: String,
    pub workspace: Option<String>,
    /// Clips that belong to the group (matching the search).
    pub count: i64,
    /// Clips from other groups pasted into this workspace.
    pub incoming: i64,
    pub last_seen: i64,
}

#[derive(Debug, Clone, Default)]
pub enum GroupSel {
    #[default]
    All,
    Ungrouped,
    /// Pinned clips from every group.
    Pinned,
    Id(i64),
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub text: String,
    pub group: GroupSel,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Place {
    pub key: String,
    pub label: String,
    pub kind: String,
    pub last_ts: i64,
    pub workspace: Option<String>,
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
