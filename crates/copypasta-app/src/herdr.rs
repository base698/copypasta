//! herdr pane context: which workspace/tab/pane/agent has focus, for herdr
//! running in a terminal and for the herdr desktop app.

use copypasta_core::context::{tilde, HerdrCtx};
use serde_json::Value;
use std::path::{Path, PathBuf};
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
        workspace_id: ws.as_ref().and_then(|w| s(w, "workspace_id")),
        tab: tab.as_ref().and_then(|t| s(t, "label")),
        pane_label: s(&pane, "label"),
        agent: a.and_then(|a| s(a, "agent")),
        agent_session: a.and_then(|a| a.get("agent_session")).and_then(|x| s(x, "value")),
        cwd: s(&pane, "foreground_cwd").or_else(|| s(&pane, "cwd")),
        title: a.and_then(|a| s(a, "terminal_title_stripped")).or_else(|| s(&pane, "terminal_title_stripped")),
        pane_id,
    })
}

// ---- the herdr desktop app ---------------------------------------------

/// herdr-core's CLI, which knows what runs in each of the app's terminals.
fn core_bin() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/hc"));
    home.into_iter()
        .chain(["/opt/homebrew/bin/hc", "/usr/local/bin/hc"].map(PathBuf::from))
        .find(|p| p.exists())
}

fn ask(bin: &Path, args: &[&str]) -> Option<Value> {
    let out = crate::platform::run_with_timeout(Command::new(bin).args(args), Duration::from_millis(800))?;
    serde_json::from_str(&out).ok()
}

/// The focused pane of the herdr desktop app. Its own binary (`app_exe`)
/// answers `status` and `ls` for the open window; a space is what the
/// terminal herdr calls a workspace.
pub fn app_focused_pane(app_exe: &Path) -> Option<HerdrCtx> {
    let (mut ctx, terminal) = parse_app(&ask(app_exe, &["status"])?, &ask(app_exe, &["ls"])?)?;
    // The window knows the layout; herdr-core knows the terminal's folder.
    if let Some(t) = terminal.and_then(|t| ask(&core_bin()?, &["get", &t])) {
        ctx.cwd = t.get("foreground").and_then(|f| s(f, "cwd")).or_else(|| s(&t, "cwd")).or(ctx.cwd);
        ctx.title = s(&t, "title");
    }
    Some(ctx)
}

/// The focused pane from `herdr-app status` and `herdr-app ls`, plus the id
/// of the terminal shown in it.
pub fn parse_app(status: &Value, ls: &Value) -> Option<(HerdrCtx, Option<String>)> {
    let pane_id = s(status, "focused_pane")?;
    let list = |v: &Value, key: &str| v.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    for space in list(ls, "spaces") {
        for tab in list(&space, "tabs") {
            let Some(term) = list(&tab, "terminals").into_iter().find(|t| s(t, "pane").as_deref() == Some(&pane_id)) else {
                continue;
            };
            let folder = s(&space, "folder");
            // An unnamed space goes by its folder, as it does in the app.
            let name = s(&space, "name").or_else(|| folder.as_deref().map(tilde).and_then(|f| f.rsplit('/').next().map(str::to_string)));
            let ctx = HerdrCtx {
                workspace: name.filter(|n| !n.is_empty()),
                workspace_id: s(&space, "id"),
                tab: s(&tab, "name"),
                pane_label: s(&term, "name"),
                // "claude-code" here, "claude" in the terminal herdr.
                agent: s(&term, "app").map(|a| a.strip_suffix("-code").unwrap_or(&a).to_string()),
                agent_session: None,
                cwd: folder,
                title: None,
                pane_id,
            };
            return Some((ctx, s(&term, "terminal")));
        }
    }
    // A pane the listing doesn't know (yet): still a place of its own.
    Some((HerdrCtx { pane_id, ..Default::default() }, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listing() -> Value {
        json!({ "spaces": [
            { "id": "sp1", "name": "api", "folder": "/work/api", "machine": null, "active": false, "tabs": [
                { "id": "tb1", "name": "logs", "terminals": [{ "terminal": "t1", "pane": "p1", "app": null, "name": "tail" }] } ] },
            { "id": "sp2", "name": null, "folder": "/work/site", "machine": null, "active": true, "tabs": [
                { "id": "tb2", "name": "fix login", "terminals": [
                    { "terminal": "t2", "pane": "p2", "app": "claude-code", "name": null },
                    { "terminal": "t3", "pane": "p3", "app": "codex", "name": null } ] } ] } ] })
    }

    #[test]
    fn app_focus_maps_spaces_to_workspaces() {
        let (ctx, terminal) = parse_app(&json!({ "space": "sp2", "tab": "tb2", "focused_pane": "p2" }), &listing()).unwrap();
        assert_eq!(terminal.as_deref(), Some("t2"));
        assert_eq!(ctx.pane_id, "p2");
        assert_eq!(ctx.workspace.as_deref(), Some("site"), "an unnamed space is called after its folder");
        assert_eq!(ctx.workspace_id.as_deref(), Some("sp2"));
        assert_eq!(ctx.tab.as_deref(), Some("fix login"));
        assert_eq!(ctx.agent.as_deref(), Some("claude"));
        assert_eq!(ctx.cwd.as_deref(), Some("/work/site"));

        let (shell, _) = parse_app(&json!({ "focused_pane": "p1" }), &listing()).unwrap();
        assert_eq!((shell.workspace.as_deref(), shell.agent, shell.pane_label.as_deref()), (Some("api"), None, Some("tail")));
        let (codex, _) = parse_app(&json!({ "focused_pane": "p3" }), &listing()).unwrap();
        assert_eq!(codex.agent.as_deref(), Some("codex"));

        let (unknown, terminal) = parse_app(&json!({ "focused_pane": "p9" }), &listing()).unwrap();
        assert_eq!((unknown.pane_id.as_str(), unknown.workspace, terminal), ("p9", None, None));
        assert!(parse_app(&json!({}), &listing()).is_none());
    }
}
