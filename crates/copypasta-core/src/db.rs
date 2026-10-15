use crate::context::{self, Context};
use crate::model::*;
use crate::{imaging, lineage, paths, secrets};
use anyhow::{Context as _, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS clips(
  id INTEGER PRIMARY KEY,
  hash TEXT UNIQUE NOT NULL,
  kind TEXT NOT NULL,
  text TEXT,
  preview TEXT NOT NULL,
  blob TEXT,
  width INTEGER,
  height INTEGER,
  bytes INTEGER NOT NULL DEFAULT 0,
  dhash INTEGER,
  secret INTEGER NOT NULL DEFAULT 0,
  types TEXT,
  first_seen INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  copies INTEGER NOT NULL DEFAULT 0,
  pastes INTEGER NOT NULL DEFAULT 0,
  pinned INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS clips_last_seen ON clips(last_seen);
CREATE TABLE IF NOT EXISTS events(
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  kind TEXT NOT NULL,
  clip_id INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
  ctx_key TEXT NOT NULL,
  label TEXT NOT NULL,
  ctx TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_clip ON events(clip_id, ts);
CREATE INDEX IF NOT EXISTS events_ctx ON events(ctx_key, ts);
CREATE TABLE IF NOT EXISTS links(
  parent INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
  child INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  score REAL,
  ts INTEGER NOT NULL,
  PRIMARY KEY(parent, child)
);
CREATE INDEX IF NOT EXISTS links_child ON links(child);
CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(body, labels, tokenize='trigram');
"#;

/// Schema changes after 0.1, applied in order and tracked in user_version.
const MIGRATIONS: &[&str] = &[
    // 1: groups (herdr workspaces or user-made), event workspace column.
    r#"
    CREATE TABLE IF NOT EXISTS groups(
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      source_key TEXT UNIQUE,
      created INTEGER NOT NULL
    );
    ALTER TABLE clips ADD COLUMN group_id INTEGER REFERENCES groups(id) ON DELETE SET NULL;
    ALTER TABLE clips ADD COLUMN group_manual INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE events ADD COLUMN workspace TEXT;
    UPDATE events SET workspace = json_extract(ctx, '$.herdr.workspace');
    CREATE INDEX IF NOT EXISTS clips_group ON clips(group_id, last_seen);
    CREATE INDEX IF NOT EXISTS events_ws ON events(workspace, kind, clip_id);
    "#,
    // 2: drop repeat events (same clip, kind and place within REPEAT_MS of an
    // earlier one: two app instances, or apps that write the clipboard
    // several times) and recount. Secrets are scrubbed after this runs.
    r#"
    DELETE FROM events WHERE id IN (
      SELECT b.id FROM events a JOIN events b
        ON b.clip_id = a.clip_id AND b.kind = a.kind AND b.ctx_key = a.ctx_key
       AND b.id != a.id AND b.ts >= a.ts AND b.ts - a.ts <= 1000
       AND (b.ts > a.ts OR b.id > a.id));
    UPDATE clips SET
      copies = (SELECT COUNT(*) FROM events WHERE clip_id = clips.id AND kind = 'copy'),
      pastes = (SELECT COUNT(*) FROM events WHERE clip_id = clips.id AND kind = 'paste');
    "#,
];

/// A copy or paste of the same clip in the same place this soon after the
/// last one is the same action reported twice.
pub const REPEAT_MS: i64 = 1000;

pub struct Store {
    pub(crate) conn: Connection,
    blob_dir: PathBuf,
}

const CLIP_COLS: &str = "id, hash, kind, text, preview, blob, width, height, bytes, secret, first_seen, last_seen, copies, pastes, pinned";

fn clip_from_row(r: &Row) -> rusqlite::Result<Clip> {
    Ok(Clip {
        id: r.get(0)?,
        hash: r.get(1)?,
        kind: ClipKind::parse(&r.get::<_, String>(2)?),
        text: r.get(3)?,
        preview: r.get(4)?,
        blob: r.get(5)?,
        width: r.get(6)?,
        height: r.get(7)?,
        bytes: r.get(8)?,
        secret: r.get::<_, i64>(9)? != 0,
        first_seen: r.get(10)?,
        last_seen: r.get(11)?,
        copies: r.get(12)?,
        pastes: r.get(13)?,
        pinned: r.get::<_, i64>(14)? != 0,
    })
}

pub(crate) fn event_from_row(r: &Row) -> rusqlite::Result<Event> {
    let kind: String = r.get(2)?;
    let ctx: String = r.get(6)?;
    Ok(Event {
        id: r.get(0)?,
        ts: r.get(1)?,
        kind: if kind == "paste" { EventKind::Paste } else { EventKind::Copy },
        clip_id: r.get(3)?,
        ctx_key: r.get(4)?,
        label: r.get(5)?,
        ctx: serde_json::from_str(&ctx).unwrap_or_default(),
    })
}

pub(crate) const EVENT_COLS: &str = "id, ts, kind, clip_id, ctx_key, label, ctx";

/// Largest text we keep verbatim; bigger copies are truncated.
const MAX_TEXT: usize = 1 << 20;

pub fn preview_of(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    context::truncate(&collapsed, 240)
}

impl Store {
    pub fn open() -> Result<Self> {
        Self::open_at(&paths::data_dir())
    }

    pub fn open_at(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir.join("blobs"))
            .with_context(|| format!("creating {}", dir.display()))?;
        let conn = Connection::open(dir.join("copypasta.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL; PRAGMA secure_delete=ON;")?;
        conn.execute_batch(SCHEMA)?;
        let mut store = Store { conn, blob_dir: dir.join("blobs") };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
            tx.commit()?;
            match i {
                0 => self.regroup_all()?,
                1 => {
                    self.scrub_secrets()?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn blob_path(&self, clip: &Clip) -> Option<PathBuf> {
        clip.blob.as_ref().map(|b| self.blob_dir.join(b))
    }

    pub fn read_blob(&self, clip: &Clip) -> Option<Vec<u8>> {
        std::fs::read(self.blob_path(clip)?).ok()
    }

    /// Store a copy event. Returns (clip_id, event_id).
    pub fn record_copy(&mut self, cap: &Captured, ctx: &Context, ts: i64) -> Result<(i64, i64)> {
        let clip_id = self.upsert_clip(cap, ts)?;
        if let Some(prev) = self.repeat_of(EventKind::Copy, clip_id, ctx, ts)? {
            return Ok((clip_id, prev));
        }
        self.conn.execute(
            "UPDATE clips SET copies = copies + 1, last_seen = ?2 WHERE id = ?1",
            params![clip_id, ts],
        )?;
        let event_id = self.insert_event(EventKind::Copy, clip_id, ctx, ts)?;
        lineage::link_copy(&self.conn, clip_id, event_id)?;
        self.auto_group(clip_id)?;
        Ok((clip_id, event_id))
    }

    pub fn record_paste(&mut self, clip_id: i64, ctx: &Context, ts: i64) -> Result<i64> {
        if let Some(prev) = self.repeat_of(EventKind::Paste, clip_id, ctx, ts)? {
            return Ok(prev);
        }
        self.conn.execute(
            "UPDATE clips SET pastes = pastes + 1, last_seen = ?2 WHERE id = ?1",
            params![clip_id, ts],
        )?;
        let id = self.insert_event(EventKind::Paste, clip_id, ctx, ts)?;
        // Make sure the destination workspace has a group, so the paste shows
        // up there as incoming, and let ungrouped clips join it.
        if let Some(ws) = ctx.herdr.as_ref().and_then(|h| h.workspace.as_deref()) {
            self.herdr_group(ws)?;
        }
        self.auto_group(clip_id)?;
        Ok(id)
    }

    /// The event this one would merely repeat (see `REPEAT_MS`).
    fn repeat_of(&self, kind: EventKind, clip_id: i64, ctx: &Context, ts: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM events WHERE clip_id = ?1 AND kind = ?2 AND ctx_key = ?3 AND ts BETWEEN ?4 AND ?5
                 ORDER BY ts DESC LIMIT 1",
                params![clip_id, kind.as_str(), ctx.key(), ts - REPEAT_MS, ts],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Remove what older builds recorded but shouldn't have: clips copied from
    /// password managers are deleted, and clips that look like credentials
    /// under the current rules are masked. Returns how many clips changed.
    pub fn scrub_secrets(&mut self) -> Result<usize> {
        let mut changed = 0;
        let pm: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT clip_id FROM events WHERE kind = 'copy' AND json_extract(ctx, '$.bundle_id') IS NOT NULL",
            )?;
            let ids: Vec<i64> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            let mut out = Vec::new();
            for id in ids {
                let bundles: Vec<Option<String>> = {
                    let mut s = self.conn.prepare_cached(
                        "SELECT json_extract(ctx, '$.bundle_id') FROM events WHERE clip_id = ?1 AND kind = 'copy'",
                    )?;
                    let v = s.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                    v
                };
                if bundles.iter().any(|b| secrets::is_password_manager(b.as_deref())) {
                    out.push(id);
                }
            }
            out
        };
        for id in pm {
            self.delete_clip(id)?;
            changed += 1;
        }
        let plain: Vec<(i64, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, text FROM clips WHERE secret = 0 AND kind != 'image' AND text IS NOT NULL")?;
            let v = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            v
        };
        for (id, text) in plain {
            if secrets::looks_secret(&text) {
                self.conn.execute(
                    "UPDATE clips SET secret = 1, text = NULL, preview = ?2 WHERE id = ?1",
                    params![id, secrets::mask(&text)],
                )?;
                self.conn.execute("DELETE FROM links WHERE parent = ?1 OR child = ?1", [id])?;
                self.refresh_fts(id)?;
                changed += 1;
            }
        }
        if changed > 0 {
            // Rewrite the file so no freed page or old index segment still
            // holds the plain text.
            self.conn.execute_batch(
                "INSERT INTO fts(fts) VALUES('optimize'); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
            )?;
        }
        Ok(changed)
    }

    fn insert_event(&mut self, kind: EventKind, clip_id: i64, ctx: &Context, ts: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO events(ts, kind, clip_id, ctx_key, label, ctx, workspace) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                ts,
                kind.as_str(),
                clip_id,
                ctx.key(),
                ctx.label(),
                serde_json::to_string(ctx)?,
                ctx.herdr.as_ref().and_then(|h| h.workspace.clone())
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.refresh_fts(clip_id)?;
        Ok(id)
    }

    /// Insert the clip if new (writing its image blob) and return its id.
    pub fn upsert_clip(&mut self, cap: &Captured, ts: i64) -> Result<i64> {
        let hash = clip_hash(cap);
        if let Some(id) = self
            .conn
            .query_row("SELECT id FROM clips WHERE hash = ?1", [&hash], |r| r.get(0))
            .optional()?
        {
            return Ok(id);
        }
        let mut text = cap.text.clone();
        if let Some(t) = &mut text {
            if t.len() > MAX_TEXT {
                let mut cut = MAX_TEXT;
                while !t.is_char_boundary(cut) {
                    cut -= 1;
                }
                t.truncate(cut);
            }
        }
        let secret = cap.kind != ClipKind::Image
            && text.as_deref().map(secrets::looks_secret).unwrap_or(false);
        let (preview, stored_text) = match cap.kind {
            ClipKind::Image => {
                let dims = match (cap.width, cap.height) {
                    (Some(w), Some(h)) => format!(" {w}×{h}"),
                    _ => String::new(),
                };
                let src = text.as_deref().map(|u| format!(" · {}", context::truncate(u, 120))).unwrap_or_default();
                (format!("Image{dims}{src}"), text.clone())
            }
            _ if secret => (secrets::mask(text.as_deref().unwrap_or("")), None),
            _ => (preview_of(text.as_deref().unwrap_or("")), text.clone()),
        };
        let (blob, bytes, dhash) = match &cap.png {
            Some(png) => {
                let name = format!("{hash}.png");
                std::fs::write(self.blob_dir.join(&name), png)?;
                (Some(name), png.len() as i64, imaging::dhash(png).map(|h| h as i64))
            }
            None => (None, stored_text.as_ref().map(|t| t.len() as i64).unwrap_or(0), None),
        };
        self.conn.execute(
            "INSERT INTO clips(hash, kind, text, preview, blob, width, height, bytes, dhash, secret, types, first_seen, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![
                hash,
                cap.kind.as_str(),
                stored_text,
                preview,
                blob,
                cap.width,
                cap.height,
                bytes,
                dhash,
                secret as i64,
                serde_json::to_string(&cap.types)?,
                ts
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn refresh_fts(&self, clip_id: i64) -> Result<()> {
        let (text, preview, secret): (Option<String>, String, i64) = self.conn.query_row(
            "SELECT text, preview, secret FROM clips WHERE id = ?1",
            [clip_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let body = if secret != 0 { preview.clone() } else { text.unwrap_or(preview) };
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT kind || ' ' || label || COALESCE(' ' || workspace, '') FROM events WHERE clip_id = ?1",
        )?;
        let labels: Vec<String> = stmt
            .query_map([clip_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        self.conn.execute("DELETE FROM fts WHERE rowid = ?1", [clip_id])?;
        self.conn.execute(
            "INSERT INTO fts(rowid, body, labels) VALUES (?1, ?2, ?3)",
            params![clip_id, body, labels.join("\n")],
        )?;
        Ok(())
    }

    pub fn clip(&self, id: i64) -> Result<Option<Clip>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {CLIP_COLS} FROM clips WHERE id = ?1"), [id], clip_from_row)
            .optional()?)
    }

    pub fn clip_by_text(&self, text: &str) -> Result<Option<Clip>> {
        let cap = Captured { text: Some(text.to_string()), ..Default::default() };
        Ok(self
            .conn
            .query_row(&format!("SELECT {CLIP_COLS} FROM clips WHERE hash = ?1"), [clip_hash(&cap)], clip_from_row)
            .optional()?)
    }

    pub fn clip_id_by_hash(&self, hash: &str) -> Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT id FROM clips WHERE hash = ?1", [hash], |r| r.get(0)).optional()?)
    }

    /// Clips matching every whitespace-separated term in body or place labels,
    /// newest first. An empty query lists recent clips.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ClipRow>> {
        self.query(&Query { text: query.into(), limit, ..Default::default() })
    }

    /// Paged, optionally group-filtered search. In a herdr workspace group,
    /// clips from other groups that were pasted into that workspace are
    /// included and marked `incoming`.
    pub fn query(&self, q: &Query) -> Result<Vec<ClipRow>> {
        let f = self.filter(&q.text)?;
        let mut sql = format!("SELECT {CLIP_COLS} FROM clips c WHERE {}", f.sql);
        let mut args = f.args.clone();
        let mut focus: Option<(i64, Option<String>)> = None;
        match &q.group {
            GroupSel::All => {}
            GroupSel::Ungrouped => sql.push_str(" AND c.group_id IS NULL"),
            GroupSel::Id(g) => {
                let ws = self.group(*g)?.and_then(|g| g.workspace);
                args.push(g.to_string());
                let n = args.len();
                match &ws {
                    Some(w) => {
                        args.push(w.clone());
                        sql.push_str(&format!(
                            " AND (c.group_id = ?{n} OR c.id IN (SELECT clip_id FROM events WHERE kind = 'paste' AND workspace = ?{}))",
                            n + 1
                        ));
                    }
                    None => sql.push_str(&format!(" AND c.group_id = ?{n}")),
                }
                focus = Some((*g, ws));
            }
        }
        let limit = if q.limit == 0 { 50 } else { q.limit.min(500) };
        sql.push_str(&format!(" ORDER BY c.pinned DESC, c.last_seen DESC LIMIT {limit} OFFSET {}", q.offset));
        let mut stmt = self.conn.prepare(&sql)?;
        let clips: Vec<Clip> = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), clip_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        let mut rows = self.rows(clips)?;
        if let Some((g, _)) = focus {
            for r in &mut rows {
                r.incoming = r.group.as_ref().map(|x| x.id) != Some(g);
            }
        }
        Ok(rows)
    }

    /// SQL condition over `c` (clips) for a search string: FTS terms plus
    /// `group:name` / `g:name` / `g:none` filters.
    pub(crate) fn filter(&self, text: &str) -> Result<Filter> {
        let mut sql = String::from("1=1");
        let mut args: Vec<String> = Vec::new();
        let mut terms: Vec<&str> = Vec::new();
        for t in text.split_whitespace() {
            let lower = t.to_lowercase();
            if let Some(name) = lower.strip_prefix("group:").or_else(|| lower.strip_prefix("g:")) {
                if name == "none" || name == "ungrouped" {
                    sql.push_str(" AND c.group_id IS NULL");
                } else {
                    args.push(format!("%{}%", name.replace('%', "\\%").replace('_', "\\_")));
                    sql.push_str(&format!(
                        " AND c.group_id IN (SELECT id FROM groups WHERE lower(name) LIKE ?{} ESCAPE '\\')",
                        args.len()
                    ));
                }
            } else {
                terms.push(t);
            }
        }
        let long: Vec<&str> = terms.iter().copied().filter(|t| t.chars().count() >= 3).collect();
        if !long.is_empty() {
            let m = long
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" AND ");
            args.push(m);
            sql.push_str(&format!(" AND c.id IN (SELECT rowid FROM fts WHERE fts MATCH ?{})", args.len()));
        }
        for t in terms.iter().filter(|t| t.chars().count() < 3) {
            args.push(format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")));
            let n = args.len();
            sql.push_str(&format!(
                " AND c.id IN (SELECT rowid FROM fts WHERE body LIKE ?{n} ESCAPE '\\' OR labels LIKE ?{n} ESCAPE '\\')"
            ));
        }
        Ok(Filter { sql, args })
    }

    pub fn rows(&self, clips: Vec<Clip>) -> Result<Vec<ClipRow>> {
        clips
            .into_iter()
            .map(|clip| {
                let (sources, destinations) = self.places(clip.id)?;
                let group = self.clip_group(clip.id)?;
                let mut workspaces: Vec<String> = Vec::new();
                for p in sources.iter().chain(&destinations) {
                    if let Some(w) = &p.workspace {
                        if !workspaces.contains(w) {
                            workspaces.push(w.clone());
                        }
                    }
                }
                let edit_of = self
                    .conn
                    .prepare_cached("SELECT parent FROM links WHERE child = ?1 AND kind = 'similar' ORDER BY score DESC LIMIT 1")?
                    .query_row([clip.id], |r| r.get(0))
                    .optional()?;
                Ok(ClipRow { clip, sources, destinations, group, incoming: false, workspaces, edit_of })
            })
            .collect()
    }

    /// Distinct copy sources and paste destinations, most recent first.
    pub fn places(&self, clip_id: i64) -> Result<(Vec<Place>, Vec<Place>)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT kind, ctx_key, label, ctx, MAX(ts), workspace FROM events WHERE clip_id = ?1
             GROUP BY kind, ctx_key ORDER BY MAX(ts) DESC",
        )?;
        let mut src = Vec::new();
        let mut dst = Vec::new();
        let rows = stmt.query_map([clip_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        for row in rows {
            let (kind, key, label, ctx, ts, workspace) = row?;
            let ctx: Context = serde_json::from_str(&ctx).unwrap_or_default();
            let place = Place { key, label, kind: context::kind(&ctx).into(), last_ts: ts, workspace };
            if kind == "paste" { dst.push(place) } else { src.push(place) }
        }
        Ok((src, dst))
    }

    pub fn events_for_clip(&self, clip_id: i64) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {EVENT_COLS} FROM events WHERE clip_id = ?1 ORDER BY ts"))?;
        let ev = stmt.query_map([clip_id], event_from_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(ev)
    }

    /// (parent, child, kind, score) links touching any of `ids`.
    pub fn links_touching(&self, ids: &[i64]) -> Result<Vec<(i64, i64, String, f64)>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT parent, child, kind, COALESCE(score, 0) FROM links WHERE parent IN ({list}) OR child IN ({list})"
        ))?;
        let v = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn delete_clip(&mut self, id: i64) -> Result<()> {
        if let Some(c) = self.clip(id)? {
            if let Some(p) = self.blob_path(&c) {
                let _ = std::fs::remove_file(p);
            }
        }
        self.conn.execute("DELETE FROM fts WHERE rowid = ?1", [id])?;
        self.conn.execute("DELETE FROM clips WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_pinned(&mut self, id: i64, pinned: bool) -> Result<()> {
        self.conn.execute("UPDATE clips SET pinned = ?2 WHERE id = ?1", params![id, pinned as i64])?;
        Ok(())
    }

    /// Drop unpinned clips older than `max_age_ms`, then the oldest until the
    /// data directory is under `max_bytes`. Returns how many clips were removed.
    pub fn gc(&mut self, now: i64, max_age_ms: i64, max_bytes: i64) -> Result<usize> {
        let old: Vec<i64> = {
            let mut stmt = self.conn.prepare("SELECT id FROM clips WHERE pinned = 0 AND last_seen < ?1")?;
            let v = stmt.query_map([now - max_age_ms], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            v
        };
        let mut removed = 0;
        for id in old {
            self.delete_clip(id)?;
            removed += 1;
        }
        let mut total = self.total_bytes()?;
        if total > max_bytes {
            let oldest: Vec<(i64, i64)> = {
                let mut stmt = self
                    .conn
                    .prepare("SELECT id, bytes FROM clips WHERE pinned = 0 ORDER BY last_seen ASC")?;
                let v = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
                v
            };
            for (id, bytes) in oldest {
                if total <= max_bytes {
                    break;
                }
                self.delete_clip(id)?;
                total -= bytes.max(1);
                removed += 1;
            }
        }
        if removed > 0 {
            self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(removed)
    }

    /// Database file size plus image blobs on disk.
    pub fn total_bytes(&self) -> Result<i64> {
        let pages: i64 = self.conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let size: i64 = self.conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let images: i64 = self
            .conn
            .query_row("SELECT COALESCE(SUM(bytes),0) FROM clips WHERE blob IS NOT NULL", [], |r| r.get(0))?;
        // Text payload already lives inside the database pages.
        Ok(pages * size + images)
    }

    pub fn stats(&self) -> Result<HashMap<&'static str, i64>> {
        let mut m = HashMap::new();
        for (k, sql) in [
            ("clips", "SELECT COUNT(*) FROM clips"),
            ("copies", "SELECT COUNT(*) FROM events WHERE kind='copy'"),
            ("pastes", "SELECT COUNT(*) FROM events WHERE kind='paste'"),
            ("links", "SELECT COUNT(*) FROM links"),
            ("images", "SELECT COUNT(*) FROM clips WHERE kind='image'"),
            ("groups", "SELECT COUNT(*) FROM groups"),
        ] {
            m.insert(k, self.conn.query_row(sql, [], |r| r.get(0))?);
        }
        m.insert("bytes", self.total_bytes()?);
        Ok(m)
    }
}

pub fn clip_hash(cap: &Captured) -> String {
    let mut h = Sha256::new();
    match &cap.png {
        Some(png) if cap.kind == ClipKind::Image => {
            h.update(b"image\0");
            h.update(png);
        }
        _ => {
            h.update(b"text\0");
            h.update(cap.text.as_deref().unwrap_or("").as_bytes());
        }
    }
    hex::encode(h.finalize())
}

pub(crate) struct Filter {
    pub sql: String,
    pub args: Vec<String>,
}
