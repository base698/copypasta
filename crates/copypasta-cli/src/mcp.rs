//! Minimal MCP server (JSON-RPC 2.0 over newline-delimited stdio).

use anyhow::Result;
use copypasta_core::{context::truncate, graph::fmt_ts, Store};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";

fn tools() -> Value {
    json!([
        {
            "name": "clipboard_trace",
            "description": "Where did this text come from? Looks up the user's clipboard history for the given text (or clip id) and returns its copy/paste lineage: the windows it was copied from and pasted into (browser URL, Slack channel, herdr pane and agent), with times, plus related clips it was quoted from or derived from.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to trace (exact or partial)." },
                    "clip_id": { "type": "integer", "description": "A clip id from clipboard_search." }
                }
            }
        },
        {
            "name": "clipboard_search",
            "description": "Search the user's clipboard history (last 30 days). Matches clip text and the places it was copied from / pasted into, e.g. 'slack dev', 'docs.rs', 'herdr claude'. Add g:<name> to limit to a group (herdr workspaces are groups automatically; g:none = ungrouped), or is:pinned for clips the user pinned. Empty query lists the most recent clips. Page with offset.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "default": 15, "maximum": 100 },
                    "offset": { "type": "integer", "default": 0 }
                }
            }
        },
        {
            "name": "clipboard_groups",
            "description": "List clipboard groups: one per herdr workspace (automatic) plus any the user made, with clip counts and how many clips from other workspaces were pasted in. Optional query narrows to groups with matching clips.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } } }
        }
    ])
}

fn call(store: &Store, name: &str, args: &Value) -> Result<String> {
    match name {
        "clipboard_trace" => {
            let q = match (args.get("clip_id").and_then(Value::as_i64), args.get("text").and_then(Value::as_str)) {
                (Some(id), _) => format!("#{id}"),
                (None, Some(t)) => t.to_string(),
                _ => anyhow::bail!("pass `text` or `clip_id`"),
            };
            crate::trace::trace(store, &q)
        }
        "clipboard_search" => {
            let q = args.get("query").and_then(Value::as_str).unwrap_or("");
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(15).min(100) as usize;
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let rows = store.query(&copypasta_core::Query { text: q.into(), limit, offset, ..Default::default() })?;
            if rows.is_empty() {
                return Ok("No matching clips.".into());
            }
            let mut out = String::new();
            for r in rows {
                let src: Vec<&str> = r.sources.iter().take(2).map(|p| p.label.as_str()).collect();
                let dst: Vec<&str> = r.destinations.iter().take(4).map(|p| p.label.as_str()).collect();
                out.push_str(&format!(
                    "#{} [{}]{}{} {} — {}\n  from: {}\n  into: {}\n",
                    r.clip.id,
                    r.clip.kind.as_str(),
                    if r.clip.pinned { " pinned" } else { "" },
                    r.group.as_ref().map(|g| format!(" group={}", g.name)).unwrap_or_default(),
                    fmt_ts(r.clip.last_seen),
                    truncate(&r.clip.preview, 200),
                    if src.is_empty() { "?".into() } else { src.join("; ") },
                    if dst.is_empty() { "(not pasted)".into() } else { dst.join("; ") },
                ));
            }
            Ok(out)
        }
        "clipboard_groups" => {
            let q = args.get("query").and_then(Value::as_str).unwrap_or("");
            let mut out = String::new();
            for g in store.group_summaries(q)? {
                out.push_str(&format!(
                    "{}{}: {} clips{}, last active {}\n",
                    g.name,
                    g.workspace.as_ref().map(|w| format!(" (herdr workspace {w})")).unwrap_or_default(),
                    g.count,
                    if g.incoming > 0 { format!(", {} pasted in from other groups", g.incoming) } else { String::new() },
                    fmt_ts(g.last_seen)
                ));
            }
            Ok(if out.is_empty() { "No groups.".into() } else { out })
        }
        _ => anyhow::bail!("unknown tool {name}"),
    }
}

pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut store: Option<Store> = None;
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue }; // notification
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result: std::result::Result<Value, (i64, String)> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "copypasta", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Clipboard history with provenance. Use clipboard_trace when the user asks where some text, a snippet or an error came from, or what they pasted where; use clipboard_search to find something they copied."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if store.is_none() {
                    store = Store::open().ok();
                }
                let out = match &store {
                    Some(s) => call(s, name, &args),
                    None => Err(anyhow::anyhow!("clipboard database unavailable")),
                };
                Ok(match out {
                    Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                    Err(e) => json!({ "content": [{ "type": "text", "text": format!("error: {e:#}") }], "isError": true }),
                })
            }
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        let resp = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        };
        writeln!(stdout, "{resp}")?;
        stdout.flush()?;
    }
    Ok(())
}
