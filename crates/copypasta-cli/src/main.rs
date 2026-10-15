//! copypasta — command-line companion to the Copypasta app.
//!
//!   copypasta tui                 interactive history + lineage (herdr tab)
//!   copypasta mcp                 MCP server on stdio
//!   copypasta search [query…]     list matching clips (recent when empty)
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
        "usage: copypasta <tui | mcp | search [query…] | trace <text|#id|-> | stats | gc>\n\
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
                    "#{:<5} {}  {}\n       from {}{}",
                    r.clip.id,
                    fmt_ts(r.clip.last_seen),
                    truncate(&r.clip.preview, 90),
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
        // Machine-readable dumps: `json search <q>` / `json graph <id>`.
        "json" => {
            let store = Store::open()?;
            let sub = args.get(1).map(String::as_str).unwrap_or("");
            let arg = args.get(2..).unwrap_or(&[]).join(" ");
            let v = match sub {
                "search" => serde_json::to_value(store.search(&arg, 200)?)?,
                "graph" => serde_json::to_value(copypasta_core::graph::build(&store, arg.trim().parse()?)?)?,
                _ => usage(),
            };
            println!("{v}");
            Ok(())
        }
        "-h" | "--help" | "help" => usage(),
        _ => usage(),
    }
}
