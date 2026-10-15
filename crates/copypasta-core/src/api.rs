//! One JSON entry point for everything the panel does with the store, so
//! the Tauri app, the CLI (`copypasta api`) and the UI test harness share it.

use crate::model::*;
use crate::{graph, imaging, Store};
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use serde_json::{json, Value};

fn id(args: &Value, key: &str) -> Result<i64> {
    args.get(key).and_then(Value::as_i64).ok_or_else(|| anyhow!("missing integer `{key}`"))
}

fn text<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `null` / "all" → all, "none" → ungrouped, number → that group.
fn group_sel(v: Option<&Value>) -> Result<GroupSel> {
    Ok(match v {
        None | Some(Value::Null) => GroupSel::All,
        Some(Value::String(s)) if s == "all" => GroupSel::All,
        Some(Value::String(s)) if s == "none" => GroupSel::Ungrouped,
        Some(v) => GroupSel::Id(v.as_i64().ok_or_else(|| anyhow!("bad group {v}"))?),
    })
}

pub fn call(store: &mut Store, cmd: &str, args: &Value) -> Result<Value> {
    Ok(match cmd {
        "search" => {
            let q = Query {
                text: text(args, "query").to_string(),
                group: group_sel(args.get("group"))?,
                offset: args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                limit: args.get("limit").and_then(Value::as_u64).unwrap_or(40) as usize,
            };
            serde_json::to_value(store.query(&q)?)?
        }
        "groups" => serde_json::to_value(store.group_summaries(text(args, "query"))?)?,
        "all_groups" => serde_json::to_value(store.all_groups()?)?,
        "clip" => {
            let c = store.clip(id(args, "id")?)?.ok_or_else(|| anyhow!("no such clip"))?;
            serde_json::to_value(store.rows(vec![c])?.pop())?
        }
        "graph" => serde_json::to_value(graph::build(store, id(args, "id")?)?)?,
        "image" => {
            let Some(clip) = store.clip(id(args, "id")?)? else { return Ok(Value::Null) };
            let Some(png) = store.read_blob(&clip) else { return Ok(Value::Null) };
            let png = match args.get("max").and_then(Value::as_u64) {
                Some(m) => imaging::thumbnail(&png, m as u32)?,
                None => png,
            };
            json!(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
        }
        "delete" => {
            store.delete_clip(id(args, "id")?)?;
            Value::Null
        }
        "pin" => {
            store.set_pinned(id(args, "id")?, args.get("pinned").and_then(Value::as_bool).unwrap_or(true))?;
            Value::Null
        }
        "move" => {
            let ids: Vec<i64> = match args.get("ids").and_then(Value::as_array) {
                Some(a) => a.iter().filter_map(Value::as_i64).collect(),
                None => vec![id(args, "id")?],
            };
            let group = match args.get("group") {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.as_i64().ok_or_else(|| anyhow!("bad group {v}"))?),
            };
            store.move_clips(&ids, group)?;
            Value::Null
        }
        "reset_group" => {
            store.reset_group(id(args, "id")?)?;
            Value::Null
        }
        "create_group" => {
            let gid = store.create_group(text(args, "name"))?;
            if let Some(ids) = args.get("ids").and_then(Value::as_array) {
                let ids: Vec<i64> = ids.iter().filter_map(Value::as_i64).collect();
                store.move_clips(&ids, Some(gid))?;
            }
            json!(gid)
        }
        "rename_group" => {
            store.rename_group(id(args, "id")?, text(args, "name"))?;
            Value::Null
        }
        "delete_group" => {
            store.delete_group(id(args, "id")?)?;
            Value::Null
        }
        "stats" => serde_json::to_value(store.stats()?)?,
        _ => bail!("unknown command {cmd}"),
    })
}
