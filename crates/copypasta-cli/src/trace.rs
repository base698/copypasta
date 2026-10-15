//! "Where did this come from?" — find the clip for some text and describe
//! its lineage.

use anyhow::Result;
use copypasta_core::{context::truncate, graph, Clip, Store};

/// Best clip for `query`: `#123` (or bare `123`) id, exact text, then the newest clip that
/// contains (or is contained in) the text, then any search hit.
pub fn find(store: &Store, query: &str) -> Result<(Option<Clip>, Vec<Clip>)> {
    let q = query.trim();
    if let Some(id) = q.strip_prefix('#').and_then(|n| n.parse::<i64>().ok()) {
        return Ok((store.clip(id)?, vec![]));
    }
    // A bare number is an id too, unless it is also something that was copied.
    if let Ok(id) = q.parse::<i64>() {
        if store.clip_by_text(q)?.is_none() {
            if let Some(c) = store.clip(id)? {
                return Ok((Some(c), vec![]));
            }
        }
    }
    if let Some(c) = store.clip_by_text(query)?.or(store.clip_by_text(q)?) {
        return Ok((Some(c), vec![]));
    }
    // Search on the first dozen words; long pastes rarely match verbatim.
    let probe: String = q.split_whitespace().take(12).collect::<Vec<_>>().join(" ");
    let mut hits: Vec<Clip> = store.search(&probe, 20)?.into_iter().map(|r| r.clip).collect();
    if hits.is_empty() {
        // Fall back to the most distinctive (longest) words.
        let mut words: Vec<&str> = q.split_whitespace().filter(|w| w.chars().count() >= 4).collect();
        words.sort_by_key(|w| std::cmp::Reverse(w.len()));
        words.truncate(3);
        hits = store.search(&words.join(" "), 20)?.into_iter().map(|r| r.clip).collect();
    }
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let nq = norm(q);
    let best = hits
        .iter()
        .position(|c| c.text.as_deref().map(|t| { let t = norm(t); t.contains(&nq) || (t.len() >= 16 && nq.contains(&t)) }).unwrap_or(false))
        .unwrap_or(0);
    if hits.is_empty() {
        return Ok((None, vec![]));
    }
    let chosen = hits.remove(best);
    Ok((Some(chosen), hits))
}

pub fn trace(store: &Store, query: &str) -> Result<String> {
    let (clip, others) = find(store, query)?;
    let Some(clip) = clip else {
        return Ok("No clipboard history matches that text (history covers the last 30 days).".into());
    };
    let g = graph::build(store, clip.id)?;
    let (src, dst) = store.places(clip.id)?;
    let row = store.rows(vec![clip.clone()])?.pop();
    let mut out = format!(
        "Clip #{} ({}): {}\nfirst copied {} · copied {}× · pasted {}×\n",
        clip.id,
        clip.kind.as_str(),
        truncate(&clip.preview, 160),
        graph::fmt_ts(clip.first_seen),
        clip.copies,
        clip.pastes
    );
    if let Some(g) = row.as_ref().and_then(|r| r.group.as_ref()) {
        out.push_str(&format!("group: {}{}\n", g.name, if g.manual { " (set by user)" } else { "" }));
    }
    if let Some(ws) = row.as_ref().map(|r| &r.workspaces).filter(|w| w.len() > 1) {
        out.push_str(&format!("crossed herdr workspaces: {}\n", ws.join(", ")));
    }
    if !src.is_empty() {
        out.push_str(&format!("copied from: {}\n", src.iter().map(|p| p.label.as_str()).collect::<Vec<_>>().join("; ")));
    }
    if !dst.is_empty() {
        out.push_str(&format!("pasted into: {}\n", dst.iter().map(|p| p.label.as_str()).collect::<Vec<_>>().join("; ")));
    }
    out.push_str("\nRelated clips, oldest first (↳ says how each relates to an earlier one):\n");
    out.push_str(&graph::render_text(&g));
    if !others.is_empty() {
        out.push_str("\nOther possible matches:\n");
        for c in others.iter().take(5) {
            out.push_str(&format!("  #{} {}\n", c.id, truncate(&c.preview, 100)));
        }
    }
    Ok(out)
}
