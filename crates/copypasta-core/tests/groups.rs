use copypasta_core::context::HerdrCtx;
use copypasta_core::{api, Captured, ClipKind, Context, GroupSel, Query, Store};
use serde_json::json;

fn text(t: &str) -> Captured {
    Captured { kind: ClipKind::Text, text: Some(t.into()), ..Default::default() }
}
fn pane(ws: &str, pane: &str, agent: Option<&str>) -> Context {
    Context {
        app_name: Some("Ghostty".into()),
        herdr: Some(HerdrCtx {
            pane_id: pane.into(),
            workspace: Some(ws.into()),
            agent: agent.map(Into::into),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn chrome() -> Context {
    Context { app_name: Some("Google Chrome".into()), url: Some("https://docs.rs/x".into()), ..Default::default() }
}
fn tempdir() -> std::path::PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let d = std::env::temp_dir().join(format!("copypasta-groups-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
fn group_name(s: &Store, clip: i64) -> Option<String> {
    s.rows(vec![s.clip(clip).unwrap().unwrap()]).unwrap()[0].group.as_ref().map(|g| g.name.clone())
}
fn ids(rows: &[copypasta_core::ClipRow]) -> Vec<i64> {
    rows.iter().map(|r| r.clip.id).collect()
}

#[test]
fn copies_group_by_workspace_and_pastes_adopt() {
    let mut s = Store::open_at(&tempdir()).unwrap();
    let (a, _) = s.record_copy(&text("cargo test -p api --lib limiter"), &pane("api", "w1:p1", None), 1).unwrap();
    assert_eq!(group_name(&s, a).as_deref(), Some("api"));

    // Copied in Chrome: ungrouped until pasted into a workspace.
    let (u, _) = s.record_copy(&text("https://docs.rs/tokio/latest/tokio"), &chrome(), 2).unwrap();
    assert_eq!(group_name(&s, u), None);
    s.record_paste(u, &pane("web", "w2:p1", Some("claude")), 3).unwrap();
    assert_eq!(group_name(&s, u).as_deref(), Some("web"));
    // A later paste elsewhere doesn't move it.
    s.record_paste(u, &pane("api", "w1:p1", None), 4).unwrap();
    assert_eq!(group_name(&s, u).as_deref(), Some("web"));
}

#[test]
fn cross_workspace_paste_is_incoming_and_answers_belong_to_destination() {
    let mut s = Store::open_at(&tempdir()).unwrap();
    let (err, _) = s
        .record_copy(&text("panicked at src/limiter.rs:88: called unwrap on None"), &pane("api", "w1:p2", None), 1_000)
        .unwrap();
    // Pasted into an agent in a different workspace.
    s.record_paste(err, &pane("research", "w3:p1", Some("claude")), 2_000).unwrap();
    assert_eq!(group_name(&s, err).as_deref(), Some("api"), "origin wins");

    let (ans, _) = s
        .record_copy(&text("Return a LimitError instead of unwrapping the bucket lookup."), &pane("research", "w3:p1", Some("claude")), 3_000)
        .unwrap();
    assert_eq!(group_name(&s, ans).as_deref(), Some("research"));
    let links = s.links_touching(&[ans]).unwrap();
    assert!(links.iter().any(|l| l.0 == err && l.2 == "via_context"), "{links:?}");

    let research = s.all_groups().unwrap().into_iter().find(|g| g.name == "research").unwrap();
    let rows = s.query(&Query { group: GroupSel::Id(research.id), limit: 50, ..Default::default() }).unwrap();
    let incoming: Vec<(i64, bool)> = rows.iter().map(|r| (r.clip.id, r.incoming)).collect();
    assert!(incoming.contains(&(err, true)) && incoming.contains(&(ans, false)), "{incoming:?}");
    let row = rows.iter().find(|r| r.clip.id == err).unwrap();
    assert_eq!(row.workspaces, vec!["api".to_string(), "research".to_string()]);

    let sums = s.group_summaries("").unwrap();
    let r = sums.iter().find(|g| g.name == "research").unwrap();
    assert_eq!((r.count, r.incoming), (1, 1));
    let a = sums.iter().find(|g| g.name == "api").unwrap();
    assert_eq!((a.count, a.incoming), (1, 0));
}

#[test]
fn manual_moves_rename_delete_and_reset() {
    let dir = tempdir();
    let mut s = Store::open_at(&dir).unwrap();
    let (a, _) = s.record_copy(&text("first thing copied in api"), &pane("api", "w1:p1", None), 1).unwrap();
    let (b, _) = s.record_copy(&text("second thing copied in api"), &pane("api", "w1:p1", None), 2).unwrap();

    let gid = api::call(&mut s, "create_group", &json!({"name": "release notes", "ids": [a]})).unwrap();
    let gid = gid.as_i64().unwrap();
    assert_eq!(group_name(&s, a).as_deref(), Some("release notes"));
    // Manual choice sticks even when copied again in another workspace.
    s.record_copy(&text("first thing copied in api"), &pane("web", "w2:p1", None), 3).unwrap();
    assert_eq!(group_name(&s, a).as_deref(), Some("release notes"));

    api::call(&mut s, "rename_group", &json!({"id": gid, "name": "Release notes v0.2"})).unwrap();
    assert_eq!(group_name(&s, a).as_deref(), Some("Release notes v0.2"));

    // Move out to "no group", then back to automatic.
    api::call(&mut s, "move", &json!({"id": b, "group": null})).unwrap();
    assert_eq!(group_name(&s, b), None);
    api::call(&mut s, "reset_group", &json!({"id": b})).unwrap();
    assert_eq!(group_name(&s, b).as_deref(), Some("api"));

    api::call(&mut s, "delete_group", &json!({"id": gid})).unwrap();
    assert_eq!(group_name(&s, a), None, "clips survive their group");
    assert!(s.clip(a).unwrap().is_some());
    // Renaming an automatic group keeps it attached to the workspace.
    let api_g = s.all_groups().unwrap().into_iter().find(|g| g.name == "api").unwrap();
    s.rename_group(api_g.id, "API service").unwrap();
    let (c, _) = s.record_copy(&text("third thing copied in api"), &pane("api", "w1:p1", None), 4).unwrap();
    assert_eq!(group_name(&s, c).as_deref(), Some("API service"));

    assert!(api::call(&mut s, "move", &json!({"id": c, "group": 9999})).is_err());
    assert!(api::call(&mut s, "create_group", &json!({"name": "  "})).is_err());
}

#[test]
fn paging_and_group_filters() {
    let mut s = Store::open_at(&tempdir()).unwrap();
    for i in 0..95 {
        let ws = if i % 5 == 0 { "web" } else { "api" };
        s.record_copy(&text(&format!("log line number {i:03} from the service")), &pane(ws, "w1:p1", None), 1_000 + i).unwrap();
    }
    let mut seen = Vec::new();
    for page in 0..3 {
        let rows = s
            .query(&Query { text: "log line".into(), offset: page * 40, limit: 40, ..Default::default() })
            .unwrap();
        seen.extend(ids(&rows));
    }
    assert_eq!(seen.len(), 95);
    let mut sorted = seen.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(seen, sorted, "newest first, no duplicates across pages");

    let web = s.query(&Query { text: "g:web".into(), limit: 100, ..Default::default() }).unwrap();
    assert_eq!(web.len(), 19);
    let web_042 = s.query(&Query { text: "g:web 045".into(), limit: 100, ..Default::default() }).unwrap();
    assert_eq!(web_042.len(), 1);
    let sums = s.group_summaries("number 04").unwrap();
    let total: i64 = sums.iter().map(|g| g.count).sum();
    assert_eq!(total, 11, "004 and 040..049");
    let sums = s.group_summaries("g:web").unwrap();
    assert_eq!(sums.len(), 1);
    assert_eq!(sums[0].count, 19);
    // Empty manual groups are listed when not searching (drop targets).
    s.create_group("scratch").unwrap();
    assert!(s.group_summaries("").unwrap().iter().any(|g| g.name == "scratch" && g.count == 0));
    assert!(!s.group_summaries("log").unwrap().iter().any(|g| g.name == "scratch"));
}

#[test]
fn migrates_a_v1_database() {
    let dir = tempdir();
    {
        let conn = rusqlite::Connection::open(dir.join("copypasta.db")).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE clips(id INTEGER PRIMARY KEY, hash TEXT UNIQUE NOT NULL, kind TEXT NOT NULL, text TEXT,
              preview TEXT NOT NULL, blob TEXT, width INTEGER, height INTEGER, bytes INTEGER NOT NULL DEFAULT 0,
              dhash INTEGER, secret INTEGER NOT NULL DEFAULT 0, types TEXT, first_seen INTEGER NOT NULL,
              last_seen INTEGER NOT NULL, copies INTEGER NOT NULL DEFAULT 0, pastes INTEGER NOT NULL DEFAULT 0,
              pinned INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE events(id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, kind TEXT NOT NULL,
              clip_id INTEGER NOT NULL REFERENCES clips(id) ON DELETE CASCADE, ctx_key TEXT NOT NULL,
              label TEXT NOT NULL, ctx TEXT NOT NULL);
            CREATE TABLE links(parent INTEGER NOT NULL, child INTEGER NOT NULL, kind TEXT NOT NULL, score REAL,
              ts INTEGER NOT NULL, PRIMARY KEY(parent, child));
            CREATE VIRTUAL TABLE fts USING fts5(body, labels, tokenize='trigram');
            INSERT INTO clips(id, hash, kind, text, preview, first_seen, last_seen, copies) VALUES
              (1, 'h1', 'text', 'old clip from herdr', 'old clip from herdr', 1, 1, 1),
              (2, 'h2', 'text', 'old clip from chrome', 'old clip from chrome', 2, 2, 1),
              (4, 'h4', 'text', 'Xk2#mPq9!vRt', 'Xk2#mPq9!vRt', 4, 4, 1),
              (5, 'h5', 'text', 'hunter22', 'hunter22', 5, 5, 1);
            INSERT INTO events(ts, kind, clip_id, ctx_key, label, ctx) VALUES
              (1, 'copy', 1, 'herdr:w1:p1', 'herdr claude', '{"herdr":{"pane_id":"w1:p1","workspace":"copypasta"}}'),
              (2, 'copy', 2, 'web:x', 'example.com', '{"url":"https://example.com"}'),
              (40, 'copy', 2, 'web:x', 'example.com', '{"url":"https://example.com"}'),
              (5000, 'paste', 2, 'app:y', 'Y', '{"app_name":"Y"}'),
              (5000, 'paste', 2, 'app:y', 'Y', '{"app_name":"Y"}'),
              (9000, 'paste', 2, 'app:y', 'Y', '{"app_name":"Y"}'),
              (4, 'copy', 4, 'app:z', 'Z', '{"app_name":"Z"}'),
              (5, 'copy', 5, 'app:bw', 'Bitwarden', '{"app_name":"Bitwarden","bundle_id":"com.bitwarden.desktop"}');
            INSERT INTO fts(rowid, body, labels) VALUES (4, 'Xk2#mPq9!vRt', ''), (5, 'hunter22', '');
            "#,
        )
        .unwrap();
    }
    let s = Store::open_at(&dir).unwrap();
    assert_eq!(group_name(&s, 1).as_deref(), Some("copypasta"));
    assert_eq!(group_name(&s, 2), None);
    let v: i64 = rusqlite::Connection::open(dir.join("copypasta.db"))
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 2);
    // Repeats collapsed (two instances / double writes), real repeats kept.
    let c2 = s.clip(2).unwrap().unwrap();
    assert_eq!((c2.copies, c2.pastes), (1, 2));
    // Password-manager clips are gone; password-looking text is masked.
    assert!(s.clip(5).unwrap().is_none());
    let c4 = s.clip(4).unwrap().unwrap();
    assert!(c4.secret && c4.text.is_none() && !c4.preview.contains("vRt"));
    assert!(s.search("mPq9", 10).unwrap().is_empty());
    drop(s);
    // An older build wrote a herdr event without the workspace column.
    {
        let conn = rusqlite::Connection::open(dir.join("copypasta.db")).unwrap();
        conn.execute_batch(
            r#"INSERT INTO clips(id, hash, kind, text, preview, first_seen, last_seen, copies) VALUES (3, 'h3', 'text', 'x', 'x', 3, 3, 1);
               INSERT INTO events(ts, kind, clip_id, ctx_key, label, ctx) VALUES
               (3, 'copy', 3, 'herdr:w1:p1', 'herdr claude', '{"herdr":{"pane_id":"w1:p1","workspace":"copypasta"}}');"#,
        )
        .unwrap();
    }
    let mut s2 = Store::open_at(&dir).unwrap();
    assert_eq!(group_name(&s2, 3), None);
    assert_eq!(s2.repair().unwrap(), 1);
    assert_eq!(group_name(&s2, 3).as_deref(), Some("copypasta"));
    assert_eq!(s2.repair().unwrap(), 0);
    drop(s2);
    // Reopening is a no-op.
    let s = Store::open_at(&dir).unwrap();
    assert_eq!(s.all_groups().unwrap().len(), 1);
}
