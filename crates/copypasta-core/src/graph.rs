//! The lineage DAG for one clip. Nodes are copy/paste events (a window at a
//! moment); edges follow content from where it was copied to where it was
//! pasted, and on to whatever was copied back out of those places.
//!
//! For people, the same graph is also summarized per clip: each related clip
//! with its copy/paste steps (repeats in one place collapsed), and plain
//! relations between clips ("#6 includes #5").

use crate::context::{self, Context};
use crate::db::Store;
use crate::model::*;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet, VecDeque};

const MAX_CLIPS: usize = 30;
const MAX_DEPTH: usize = 3;
const MAX_EVENTS_PER_CLIP: usize = 25;

#[derive(Debug, Clone, Serialize)]
pub struct Graph {
    pub focus: i64,
    /// Related clips, oldest first.
    pub clips: Vec<GraphClip>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// How clips relate, child described relative to parent.
    pub relations: Vec<Relation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphClip {
    pub id: i64,
    pub kind: ClipKind,
    pub preview: String,
    pub secret: bool,
    pub first_seen: i64,
    pub steps: Vec<Step>,
}

/// One or more consecutive copies (or pastes) of a clip in the same place.
#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub kind: EventKind,
    /// herdr | slack | web | app | system
    pub place: String,
    /// Short name of the place: "claude", "shell · build", "Slack #eng".
    pub title: String,
    /// Where exactly: cwd and pane title, URL, window title.
    pub detail: Option<String>,
    pub workspace: Option<String>,
    pub ts: i64,
    pub last_ts: i64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Relation {
    pub parent: i64,
    pub child: i64,
    pub kind: String,
    /// Phrase that reads after the child's id: "includes #5".
    pub text: String,
    pub weak: bool,
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

/// Short place name for people; the herdr workspace is shown separately.
pub fn place_title(ctx: &Context, label: &str) -> String {
    if let Some(h) = &ctx.herdr {
        let who = h.agent.clone().or_else(|| h.pane_label.clone()).unwrap_or_else(|| "shell".into());
        return match h.tab.as_ref().filter(|t| !t.chars().all(|c| c.is_ascii_digit())) {
            Some(t) => format!("{who} · {t}"),
            None => who,
        };
    }
    label.to_string()
}

pub fn place_detail(ctx: &Context) -> Option<String> {
    if let Some(h) = &ctx.herdr {
        let parts: Vec<String> = [h.cwd.as_deref().map(context::tilde), h.title.as_deref().map(|t| context::truncate(t, 50))]
            .into_iter()
            .flatten()
            .collect();
        return (!parts.is_empty()).then(|| parts.join(" · "));
    }
    detail(ctx)
}

fn relation_text(kind: &str, parent: i64, place: &str) -> String {
    match kind {
        "contains" => format!("includes #{parent}"),
        "excerpt" => format!("is part of #{parent}"),
        "similar" => format!("is an edited version of #{parent}"),
        "image_similar" => format!("is the same image as #{parent}"),
        "same_url" => format!("is the image at the URL in #{parent}"),
        "via_context" => format!("was copied out of {place} after #{parent} was pasted there"),
        k => format!("{k} #{parent}"),
    }
}

fn steps_of(events: &[Event]) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::new();
    let mut last_key: Option<(EventKind, &str)> = None;
    for e in events {
        if last_key == Some((e.kind, e.ctx_key.as_str())) {
            let s = out.last_mut().expect("step for last key");
            s.count += 1;
            s.last_ts = e.ts;
            // Latest whereabouts (a shell may have changed directory).
            s.detail = place_detail(&e.ctx).or(s.detail.take());
            continue;
        }
        last_key = Some((e.kind, e.ctx_key.as_str()));
        out.push(Step {
            kind: e.kind,
            place: context::kind(&e.ctx).into(),
            title: place_title(&e.ctx, &e.label),
            detail: place_detail(&e.ctx),
            workspace: e.ctx.herdr.as_ref().and_then(|h| h.workspace.clone()),
            ts: e.ts,
            last_ts: e.ts,
            count: 1,
        });
    }
    out
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
        clips.push(GraphClip { id, kind: c.kind, preview: c.preview, secret: c.secret, first_seen: c.first_seen, steps: steps_of(&ev) });
        events.insert(id, ev);
    }
    clips.sort_by_key(|c| (c.first_seen, c.id));

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
    let mut relations = Vec::new();
    for (parent, child, kind, _) in &links {
        let (Some(pev), Some(cev)) = (events.get(parent), events.get(child)) else { continue };
        let place = cev
            .iter()
            .find(|e| e.kind == EventKind::Copy)
            .map(|e| place_title(&e.ctx, &e.label))
            .unwrap_or_else(|| "there".into());
        relations.push(Relation {
            parent: *parent,
            child: *child,
            kind: kind.clone(),
            text: relation_text(kind, *parent, &place),
            weak: kind == "via_context" || kind == "similar",
        });
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
    relations.sort_by_key(|r| (r.child, r.parent));
    Ok(Graph { focus, clips, nodes, edges, relations })
}

/// Plain-text rendering for the TUI and MCP: related clips oldest first,
/// each with where it was copied and pasted, and how it relates to the others.
pub fn render_text(g: &Graph) -> String {
    let mut out = String::new();
    for c in &g.clips {
        let me = if c.id == g.focus { "  ◀ this clip" } else { "" };
        out.push_str(&format!("#{} {}: {}{me}\n", c.id, c.kind.as_str(), context::truncate(&c.preview, 80)));
        for r in g.relations.iter().filter(|r| r.child == c.id) {
            out.push_str(&format!("   ↳ {}{}\n", r.text, if r.weak { " (probably)" } else { "" }));
        }
        for s in &c.steps {
            let verb = match s.kind {
                EventKind::Copy => "copied from",
                EventKind::Paste => "pasted into",
            };
            let times = if s.count > 1 { format!(" ×{}", s.count) } else { String::new() };
            let place = if s.place == "herdr" { format!("herdr {}", s.title) } else { s.title.clone() };
            let ws = s.workspace.as_deref().map(|w| format!(" (workspace {w})")).unwrap_or_default();
            out.push_str(&format!("   {}  {verb} {place}{ws}{times}\n", fmt_ts(s.ts)));
            if let Some(d) = &s.detail {
                out.push_str(&format!("                     {d}\n"));
            }
        }
        out.push('\n');
    }
    out
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
