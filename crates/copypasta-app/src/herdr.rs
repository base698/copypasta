//! herdr pane context: which workspace/tab/pane/agent has focus.

use copypasta_core::context::HerdrCtx;
use serde_json::Value;
use std::process::Command;
use std::time::Duration;

fn herdr_bin() -> Option<&'static str> {
    ["/opt/homebrew/bin/herdr", "/usr/local/bin/herdr", "/usr/bin/herdr"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
}

/// True when a local herdr client is attached to a terminal.
fn client_attached() -> bool {
    let Ok(out) = Command::new("/bin/ps").args(["-axo", "tty=,comm="]).output() else { return false };
    String::from_utf8_lossy(&out.stdout).lines().any(|l| {
        let mut parts = l.split_whitespace();
        let tty = parts.next().unwrap_or("??");
        let comm = parts.next().unwrap_or("");
        tty != "??" && (comm == "herdr" || comm.ends_with("/herdr"))
    })
}

pub fn focused_pane() -> Option<HerdrCtx> {
    let bin = herdr_bin()?;
    if !client_attached() {
        return None;
    }
    let out = crate::platform::run_with_timeout(
        Command::new(bin).args(["api", "snapshot"]),
        Duration::from_millis(800),
    )?;
    parse_snapshot(&serde_json::from_str(&out).ok()?)
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty())
}

pub fn parse_snapshot(v: &Value) -> Option<HerdrCtx> {
    let snap = v.get("result").and_then(|r| r.get("snapshot")).unwrap_or(v);
    let pane_id = s(snap, "focused_pane_id")?;
    let find = |list: &str, key: &str, id: &str| -> Option<Value> {
        snap.get(list)?.as_array()?.iter().find(|x| x.get(key).and_then(Value::as_str) == Some(id)).cloned()
    };
    let pane = find("panes", "pane_id", &pane_id).unwrap_or(Value::Null);
    let agent = find("agents", "pane_id", &pane_id);
    let tab = s(&pane, "tab_id").or_else(|| s(snap, "focused_tab_id")).and_then(|t| find("tabs", "tab_id", &t));
    let ws = s(&pane, "workspace_id").or_else(|| s(snap, "focused_workspace_id")).and_then(|w| find("workspaces", "workspace_id", &w));
    let a = agent.as_ref();
    Some(HerdrCtx {
        workspace: ws.as_ref().and_then(|w| s(w, "label")),
        tab: tab.as_ref().and_then(|t| s(t, "label")),
        pane_label: s(&pane, "label"),
        agent: a.and_then(|a| s(a, "agent")),
        agent_session: a.and_then(|a| a.get("agent_session")).and_then(|x| s(x, "value")),
        cwd: s(&pane, "foreground_cwd").or_else(|| s(&pane, "cwd")),
        title: a.and_then(|a| s(a, "terminal_title_stripped")).or_else(|| s(&pane, "terminal_title_stripped")),
        pane_id,
    })
}
