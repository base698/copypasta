use std::path::PathBuf;

/// ~/Library/Application Support/copypasta on macOS, $XDG_DATA_HOME/copypasta on Linux.
/// COPYPASTA_HOME overrides (used by tests).
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("COPYPASTA_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("copypasta")
}

pub fn db_path() -> PathBuf {
    data_dir().join("copypasta.db")
}

pub fn blob_dir() -> PathBuf {
    data_dir().join("blobs")
}
