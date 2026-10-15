//! copypasta — command-line companion to the Copypasta app.
//!
//!   copypasta tui                 interactive history + lineage (herdr tab)
//!   copypasta mcp                 MCP server on stdio
//!   copypasta search [query…]     list matching clips (recent when empty)
//!   copypasta groups [query…]     groups (herdr workspaces + your own) with counts
//!   copypasta api <cmd> [json]    the panel's JSON API
//!   copypasta trace <text|#id>    where did this text come from?
//!   copypasta stats | gc

mod demo;
mod mcp;
mod trace;
mod tui;

use anyhow::Result;
use copypasta_core::{context::truncate, graph::fmt_ts, Store};

fn usage() -> ! {
    eprintln!(
        "usage: copypasta <tui | mcp | search [query…] | groups [query…] | trace <text|#id|-> | api <cmd> [json] | stats | gc>\n\
         search syntax: words match text and places; g:<name> filters by group, g:none = ungrouped\n\
         data: {}",
        copypasta_core::paths::data_dir().display()
    );
    std::process::exit(2)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("tui");
    let rest = args.get(1..).unwrap_or(&[]).join(" ");
    match cmd {
        "tui" => tui::run(),
        "mcp" => mcp::serve(),
        "search" | "recent" | "ls" => {
            let store = Store::open()?;
            for r in store.search(&rest, 30)? {
                let src = r.sources.first().map(|p| p.label.as_str()).unwrap_or("?");
                let dst: Vec<&str> = r.destinations.iter().take(3).map(|p| p.label.as_str()).collect();
                println!(
                    "#{:<5} {}  {}{}\n       from {}{}",
                    r.clip.id,
                    fmt_ts(r.clip.last_seen),
                    truncate(&r.clip.preview, 90),
                    r.group.as_ref().map(|g| format!("  [{}]", g.name)).unwrap_or_default(),
                    src,
                    if dst.is_empty() { String::new() } else { format!("  →  {}", dst.join(", ")) }
                );
            }
            Ok(())
        }
        "trace" => {
            let input = if rest == "-" || rest.is_empty() {
                use std::io::Read;
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s)?;
                s
            } else {
                rest
            };
            let store = Store::open()?;
            println!("{}", trace::trace(&store, &input)?);
            Ok(())
        }
        "stats" => {
            let store = Store::open()?;
            let mut s: Vec<_> = store.stats()?.into_iter().collect();
            s.sort();
            for (k, v) in s {
                println!("{k:>7}: {v}");
            }
            Ok(())
        }
        "gc" => {
            let mut store = Store::open()?;
            let n = store.gc(copypasta_core::now_ms(), 30 * 86_400_000, 2 << 30)?;
            println!("removed {n} clips");
            Ok(())
        }
        "demo" => demo::seed(),
        // The panel's JSON API: `copypasta api <cmd> ['{"json":"args"}']`.
        "api" => {
            let mut store = Store::open()?;
            let sub = args.get(1).map(String::as_str).unwrap_or_else(|| usage());
            let json: serde_json::Value = match args.get(2) {
                Some(a) => serde_json::from_str(a)?,
                None => serde_json::Value::Null,
            };
            println!("{}", copypasta_core::api::call(&mut store, sub, &json)?);
            Ok(())
        }
        "groups" => {
            let store = Store::open()?;
            for g in store.group_summaries(&rest)? {
                println!(
                    "{:<24} {:>4} clips{}  {}",
                    g.name,
                    g.count,
                    if g.incoming > 0 { format!(" +{} incoming", g.incoming) } else { String::new() },
                    fmt_ts(g.last_seen)
                );
            }
            Ok(())
        }
        "-h" | "--help" | "help" => usage(),
        _ => usage(),
    }
}
