//! Clip-to-clip links. Each link points from an older clip (by first_seen)
//! to a newer one, so the clip graph is acyclic.
//!
//! Strong links come from content: the new clip contains an older one, is an
//! excerpt of one, is textually similar, or is a near-identical image.
//! Weak links come from place: something was pasted into an assistant (a
//! herdr pane running an agent, or a web chat) and shortly after something
//! else was copied out of that same place — e.g. a prompt pasted into Claude
//! and its answer copied back out. Secrets are never linked.

use rusqlite::{params, Connection};
use std::collections::HashSet;

const DAY_MS: i64 = 86_400_000;
/// How long after a paste a copy from the same place still counts as related.
pub const VIA_CONTEXT_WINDOW_MS: i64 = 30 * 60 * 1000;
const MIN_CONTAIN_CHARS: usize = 16;
const MAX_PARENTS: usize = 3;

struct ClipInfo {
    kind: String,
    text: Option<String>,
    dhash: Option<i64>,
    first_seen: i64,
    secret: bool,
}

fn info(conn: &Connection, id: i64) -> rusqlite::Result<ClipInfo> {
    conn.query_row(
        "SELECT kind, text, dhash, first_seen, secret FROM clips WHERE id = ?1",
        [id],
        |r| {
            Ok(ClipInfo {
                kind: r.get(0)?,
                text: r.get(1)?,
                dhash: r.get(2)?,
                first_seen: r.get(3)?,
                secret: r.get::<_, i64>(4)? != 0,
            })
        },
    )
}

pub fn link_copy(conn: &Connection, clip_id: i64, event_id: i64) -> anyhow::Result<()> {
    let (ts, ctx_key, ctx): (i64, String, String) = conn.query_row(
        "SELECT ts, ctx_key, ctx FROM events WHERE id = ?1",
        [event_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let ctx: crate::Context = serde_json::from_str(&ctx).unwrap_or_default();
    let me = info(conn, clip_id)?;
    if me.secret {
        return Ok(());
    }
    let copy_events: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events WHERE clip_id = ?1 AND kind = 'copy'",
        [clip_id],
        |r| r.get(0),
    )?;
    if copy_events == 1 {
        for (parent, kind, score) in content_parents(conn, clip_id, &me, ts)? {
            insert(conn, parent, clip_id, kind, score, ts)?;
        }
    }
    let has_strong: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM links WHERE child = ?1 AND kind != 'via_context')",
        [clip_id],
        |r| r.get(0),
    )?;
    if !has_strong && ctx.is_assistant() {
        let recent_paste: Option<i64> = conn
            .query_row(
                "SELECT e.clip_id FROM events e JOIN clips c ON c.id = e.clip_id
                 WHERE e.kind = 'paste' AND e.ctx_key = ?1 AND e.clip_id != ?2
                   AND e.ts <= ?3 AND e.ts >= ?4 AND c.first_seen < ?5
                 ORDER BY e.ts DESC LIMIT 1",
                params![ctx_key, clip_id, ts, ts - VIA_CONTEXT_WINDOW_MS, me.first_seen],
                |r| r.get(0),
            )
            .ok();
        if let Some(parent) = recent_paste {
            insert(conn, parent, clip_id, "via_context", 0.3, ts)?;
        }
    }
    Ok(())
}

fn insert(conn: &Connection, parent: i64, child: i64, kind: &str, score: f64, ts: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO links(parent, child, kind, score, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![parent, child, kind, score, ts],
    )?;
    Ok(())
}

fn content_parents(
    conn: &Connection,
    clip_id: i64,
    me: &ClipInfo,
    ts: i64,
) -> anyhow::Result<Vec<(i64, &'static str, f64)>> {
    let mut found: Vec<(i64, &'static str, f64)> = Vec::new();
    if me.kind == "image" {
        if let Some(h) = me.dhash {
            let mut stmt = conn.prepare(
                "SELECT id, dhash FROM clips WHERE kind = 'image' AND dhash IS NOT NULL AND id != ?1
                 AND first_seen < ?2 AND last_seen > ?3 ORDER BY last_seen DESC LIMIT 500",
            )?;
            let rows = stmt.query_map(params![clip_id, me.first_seen, ts - 30 * DAY_MS], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (id, other) = row?;
                let dist = ((h ^ other) as u64).count_ones();
                if dist <= 5 {
                    found.push((id, "image_similar", 1.0 - dist as f64 / 64.0));
                }
            }
        }
        // An image whose source URL was copied earlier as text.
        if let Some(url) = &me.text {
            let mut stmt = conn.prepare(
                "SELECT id FROM clips WHERE kind != 'image' AND text = ?1 AND id != ?2 AND first_seen < ?3",
            )?;
            for id in stmt.query_map(params![url, clip_id, me.first_seen], |r| r.get::<_, i64>(0))? {
                found.push((id?, "same_url", 1.0));
            }
        }
    } else if let (Some(text), false) = (&me.text, me.secret) {
        let mine = text.trim();
        let my_len = mine.chars().count();
        let my_shingles = if my_len <= 50_000 { Some(shingles(mine)) } else { None };
        let mut stmt = conn.prepare(
            "SELECT id, text FROM clips WHERE kind != 'image' AND secret = 0 AND text IS NOT NULL AND id != ?1
             AND first_seen < ?2 AND last_seen > ?3 ORDER BY last_seen DESC LIMIT 400",
        )?;
        let rows = stmt.query_map(params![clip_id, me.first_seen, ts - 7 * DAY_MS], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, other) = row?;
            let theirs = other.trim();
            let their_len = theirs.chars().count();
            if their_len >= MIN_CONTAIN_CHARS && my_len > their_len && mine.contains(theirs) {
                found.push((id, "contains", their_len as f64 / my_len as f64));
            } else if my_len >= MIN_CONTAIN_CHARS && their_len > my_len && theirs.contains(mine) {
                found.push((id, "excerpt", my_len as f64 / their_len as f64));
            } else if let Some(ms) = &my_shingles {
                if their_len <= 50_000 && my_len >= MIN_CONTAIN_CHARS && their_len >= MIN_CONTAIN_CHARS {
                    let j = jaccard(ms, &shingles(theirs));
                    if j >= 0.5 {
                        found.push((id, "similar", j));
                    }
                }
            }
        }
    }
    found.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    found.truncate(MAX_PARENTS);
    Ok(found)
}

/// Word 3-grams (falling back to single words for short text).
fn shingles(text: &str) -> HashSet<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    if words.len() < 3 {
        return words.into_iter().collect();
    }
    words.windows(3).map(|w| w.join(" ")).collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    inter / (a.len() as f64 + b.len() as f64 - inter)
}
