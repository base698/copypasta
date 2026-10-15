//! The lineage DAG for one clip. Nodes are copy/paste events (a window at a
//! moment); edges follow content from where it was copied to where it was
//! pasted, and on to whatever was copied back out of those places.

use crate::context::{self, Context};
use crate::db::Store;
use crate::model::*;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

const MAX_CLIPS: usize = 30;
const MAX_DEPTH: usize = 3;
const MAX_EVENTS_PER_CLIP: usize = 25;

#[derive(Debug, Clone, Serialize)]
pub struct Graph {
    pub focus: i64,
    pub clips: Vec<GraphClip>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphClip {
    pub id: i64,
    pub kind: ClipKind,
    pub preview: String,
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub event_id: i64,
    pub clip_id: i64,
    pub kind: EventKind,
    pub ts: i64,
    pub label: String,
    /// herdr | slack | web | app | system
    pub place: String,
    /// Secondary line: URL, herdr cwd/session, Slack workspace, window title.
    pub detail: Option<String>,
    pub ctx: Context,
}

#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// paste | recopy | contains | excerpt | similar | image_similar | same_url | via_context
    pub kind: String,
    pub weak: bool,
}

fn node_id(e: &Event) -> String {
    format!("e{}", e.id)
}

fn detail(ctx: &Context) -> Option<String> {
    if let Some(h) = &ctx.herdr {
        let mut parts = vec![format!("pane {}", h.pane_id)];
        if let Some(t) = &h.title {
            parts.push(context::truncate(t, 50));
        }
        if let Some(s) = &h.agent_session {
            parts.push(format!("session {}", &s[..s.len().min(8)]));
        }
        return Some(parts.join(" · "));
    }
    if let Some(u) = &ctx.url {
        return Some(context::truncate(u, 90));
    }
    if ctx.slack.is_some() {
        return ctx.window_title.clone();
    }
    ctx.window_title.clone().filter(|t| Some(t) != ctx.app_name.as_ref())
}

pub fn build(store: &Store, focus: i64) -> anyhow::Result<Graph> {
    // 1. Clips connected to the focus through content/context links.
    let mut seen: HashSet<i64> = HashSet::from([focus]);
    let mut queue = VecDeque::from([(focus, 0usize)]);
    let mut links: Vec<(i64, i64, String, f64)> = Vec::new();
    let mut link_seen = HashSet::new();
    while let Some((id, depth)) = queue.pop_front() {
        if depth >= MAX_DEPTH {
            continue;
        }
        for l in store.links_touching(&[id])? {
            if link_seen.insert((l.0, l.1)) {
                links.push(l.clone());
            }
            for other in [l.0, l.1] {
                if seen.len() < MAX_CLIPS && seen.insert(other) {
                    queue.push_back((other, depth + 1));
                }
            }
        }
    }
    links.retain(|l| seen.contains(&l.0) && seen.contains(&l.1));

    // 2. Events for those clips.
    let mut clips = Vec::new();
    let mut events: BTreeMap<i64, Vec<Event>> = BTreeMap::new();
    for &id in &seen {
        let Some(c) = store.clip(id)? else { continue };
        let mut ev = store.events_for_clip(id)?;
        if ev.len() > MAX_EVENTS_PER_CLIP {
            let first = ev[0].clone();
            ev = ev.split_off(ev.len() - (MAX_EVENTS_PER_CLIP - 1));
            ev.insert(0, first);
        }
        clips.push(GraphClip { id, kind: c.kind, preview: c.preview, secret: c.secret });
        events.insert(id, ev);
    }
    clips.sort_by_key(|c| c.id);

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for ev in events.values() {
        // Pastes hang off the latest copy of the same clip before them;
        // re-copies hang off a paste into the same place, else the previous copy.
        let mut last_copy: Option<&Event> = None;
        for (i, e) in ev.iter().enumerate() {
            match e.kind {
                EventKind::Paste => {
                    let parent = last_copy.or_else(|| ev[..i].last());
                    if let Some(p) = parent {
                        edges.push(Edge { from: node_id(p), to: node_id(e), kind: "paste".into(), weak: false });
                    }
                }
                EventKind::Copy => {
                    let from_paste = ev[..i]
                        .iter()
                        .rev()
                        .find(|p| p.kind == EventKind::Paste && p.ctx_key == e.ctx_key);
                    if let Some(p) = from_paste.or(last_copy) {
                        edges.push(Edge { from: node_id(p), to: node_id(e), kind: "recopy".into(), weak: false });
                    }
                    last_copy = Some(e);
                }
            }
            nodes.push(Node {
                id: node_id(e),
                event_id: e.id,
                clip_id: e.clip_id,
                kind: e.kind,
                ts: e.ts,
                label: e.label.clone(),
                place: context::kind(&e.ctx).into(),
                detail: detail(&e.ctx),
                ctx: e.ctx.clone(),
            });
        }
    }

    // 3. Clip links become edges between specific events: from the parent's
    //    paste into the place the child was copied from, else the parent's
    //    latest event before the child's first copy.
    for (parent, child, kind, _) in &links {
        let (Some(pev), Some(cev)) = (events.get(parent), events.get(child)) else { continue };
        let Some(first_copy) = cev.iter().find(|e| e.kind == EventKind::Copy).or(cev.first()) else { continue };
        let before: Vec<&Event> = pev.iter().filter(|e| e.ts <= first_copy.ts).collect();
        let src = before
            .iter()
            .rev()
            .find(|e| e.kind == EventKind::Paste && e.ctx_key == first_copy.ctx_key)
            .or(before.last())
            .copied()
            .or(pev.first());
        if let Some(s) = src {
            edges.push(Edge {
                from: node_id(s),
                to: node_id(first_copy),
                kind: kind.clone(),
                weak: kind == "via_context" || kind == "similar",
            });
        }
    }

    nodes.sort_by_key(|n| (n.ts, n.event_id));
    Ok(Graph { focus, clips, nodes, edges })
}

/// Plain-text rendering for the TUI and MCP: a time-ordered tree.
pub fn render_text(g: &Graph) -> String {
    let by_id: HashMap<&str, &Node> = g.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let clip_preview: HashMap<i64, &GraphClip> = g.clips.iter().map(|c| (c.id, c)).collect();
    let mut children: HashMap<&str, Vec<&Edge>> = HashMap::new();
    let mut has_parent = HashSet::new();
    for e in &g.edges {
        children.entry(e.from.as_str()).or_default().push(e);
        has_parent.insert(e.to.as_str());
    }
    for v in children.values_mut() {
        v.sort_by_key(|e| by_id.get(e.to.as_str()).map(|n| n.ts).unwrap_or(0));
    }
    let mut out = String::new();
    let mut printed = HashSet::new();
    let mut last_clip = None;
    for root in g.nodes.iter().filter(|n| !has_parent.contains(n.id.as_str())) {
        walk(root, None, 0, &by_id, &children, &clip_preview, &mut printed, &mut last_clip, &mut out);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    n: &'a Node,
    via: Option<&Edge>,
    depth: usize,
    by_id: &HashMap<&str, &'a Node>,
    children: &HashMap<&str, Vec<&'a Edge>>,
    clips: &HashMap<i64, &GraphClip>,
    printed: &mut HashSet<&'a str>,
    last_clip: &mut Option<i64>,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    let verb = match n.kind {
        EventKind::Copy => "copied from",
        EventKind::Paste => "pasted into",
    };
    let link = match via.map(|e| e.kind.as_str()) {
        None | Some("paste") | Some("recopy") => String::new(),
        Some(k) => format!("[{k}] "),
    };
    if *last_clip != Some(n.clip_id) {
        if let Some(c) = clips.get(&n.clip_id) {
            out.push_str(&format!("{indent}┌ clip #{} {}: {}\n", c.id, c.kind.as_str(), context::truncate(&c.preview, 80)));
        }
        *last_clip = Some(n.clip_id);
    }
    let arrow = if depth == 0 { "•" } else { "→" };
    if !printed.insert(n.id.as_str()) {
        out.push_str(&format!("{indent}{arrow} {link}{verb} {} (see above)\n", n.label));
        return;
    }
    out.push_str(&format!("{indent}{arrow} {link}{verb} {}  [{}]\n", n.label, fmt_ts(n.ts)));
    if let Some(d) = &n.detail {
        out.push_str(&format!("{indent}    {d}\n"));
    }
    if let Some(kids) = children.get(n.id.as_str()) {
        for e in kids {
            if let Some(child) = by_id.get(e.to.as_str()) {
                walk(child, Some(e), depth + 1, by_id, children, clips, printed, last_clip, out);
                *last_clip = Some(n.clip_id);
            }
        }
    }
}

/// Local "YYYY-MM-DD HH:MM" without pulling in a date crate.
pub fn fmt_ts(ms: i64) -> String {
    let secs = ms / 1000 + local_offset_secs();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn local_offset_secs() -> i64 {
    use std::sync::OnceLock;
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let s = s.trim();
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let h: i64 = s.get(1..3)?.parse().ok()?;
                let m: i64 = s.get(3..5)?.parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0)
    })
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
