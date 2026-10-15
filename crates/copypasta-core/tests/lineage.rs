use copypasta_core::context::{HerdrCtx, SlackCtx};
use copypasta_core::{graph, Captured, ClipKind, Context, Store};

fn text(t: &str) -> Captured {
    Captured { kind: ClipKind::Text, text: Some(t.into()), ..Default::default() }
}

fn chrome() -> Context {
    Context { app_name: Some("Google Chrome".into()), url: Some("https://docs.rs/tokio".into()), page_title: Some("tokio".into()), ..Default::default() }
}
fn herdr() -> Context {
    Context {
        app_name: Some("Ghostty".into()),
        herdr: Some(HerdrCtx { pane_id: "w1:p3".into(), agent: Some("claude".into()), cwd: Some("/tmp/x".into()), ..Default::default() }),
        ..Default::default()
    }
}
fn slack() -> Context {
    Context { slack: Some(SlackCtx { conversation: "#dev".into(), workspace: Some("Acme".into()), channel_id: Some("C1".into()) }), url: Some("https://app.slack.com/client/T/C1".into()), ..Default::default() }
}

#[test]
fn chrome_to_claude_to_slack() {
    let dir = tempdir();
    let mut s = Store::open_at(&dir).unwrap();
    let q = "how do I spawn a blocking task in tokio runtime";
    let (a, _) = s.record_copy(&text(q), &chrome(), 1_000).unwrap();
    s.record_paste(a, &herdr(), 2_000).unwrap();
    // Claude's answer copied out of the same pane: linked by place.
    let ans = "Use tokio::task::spawn_blocking(|| heavy_work()).await to run it off the async workers.";
    let (b, _) = s.record_copy(&text(ans), &herdr(), 60_000).unwrap();
    s.record_paste(b, &slack(), 70_000).unwrap();
    // A snippet of the answer copied from Slack: linked by content.
    let (c, _) = s.record_copy(&text("tokio::task::spawn_blocking(|| heavy_work())"), &slack(), 90_000).unwrap();

    let g = graph::build(&s, b).unwrap();
    assert_eq!(g.clips.len(), 3);
    let kinds: Vec<&str> = g.edges.iter().map(|e| e.kind.as_str()).collect();
    assert!(kinds.contains(&"via_context"), "{kinds:?}");
    assert!(kinds.contains(&"excerpt"), "{kinds:?}");
    let txt = graph::render_text(&g);
    assert!(txt.contains("herdr claude"), "{txt}");
    assert!(txt.contains("Slack #dev"), "{txt}");
    eprintln!("{txt}");

    let hits = s.search("spawn_blocking", 10).unwrap();
    assert_eq!(hits.len(), 2);
    let by_place = s.search("slack dev", 10).unwrap();
    assert!(by_place.iter().any(|r| r.clip.id == c));
    let short = s.search("do I", 10).unwrap();
    assert_eq!(short.len(), 1);
}

#[test]
fn secrets_are_masked_and_gc() {
    let dir = tempdir();
    let mut s = Store::open_at(&dir).unwrap();
    let (id, _) = s.record_copy(&text("ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz"), &herdr(), 1).unwrap();
    let c = s.clip(id).unwrap().unwrap();
    assert!(c.secret && c.text.is_none() && !c.preview.contains("abcdefghij"));
    assert!(s.search("abcdefghij", 5).unwrap().is_empty());
    s.record_copy(&text("fresh"), &chrome(), 40 * 86_400_000).unwrap();
    assert_eq!(s.gc(40 * 86_400_000, 30 * 86_400_000, 2 << 30).unwrap(), 1);
}

fn tempdir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("copypasta-test-{}-{}", std::process::id(), rand()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
fn rand() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

#[test]
fn no_context_link_from_plain_shells_or_secrets() {
    let dir = tempdir();
    let mut s = Store::open_at(&dir).unwrap();
    let shell = Context {
        herdr: Some(HerdrCtx { pane_id: "w1:p9".into(), ..Default::default() }),
        ..Default::default()
    };
    let (a, _) = s.record_copy(&text("cargo build --release -p copypasta-app"), &chrome(), 1_000).unwrap();
    s.record_paste(a, &shell, 2_000).unwrap();
    let (b, _) = s.record_copy(&text("error[E0425]: cannot find value `x` in this scope"), &shell, 3_000).unwrap();
    let (c, _) = s.record_copy(&text("TOKEN=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), &herdr(), 4_000).unwrap();
    assert!(s.links_touching(&[b, c]).unwrap().is_empty());
}

/// The notarytool session: a command copied from Claude, pasted into a shell
/// twice (each event reported twice by a duplicate instance), the shell output
/// copied back, then a fixed command. The graph should read as three clips.
#[test]
fn repeats_collapse_and_clips_relate_in_words() {
    let dir = tempdir();
    let mut s = Store::open_at(&dir).unwrap();
    let claude = herdr();
    let shell = Context {
        herdr: Some(HerdrCtx {
            pane_id: "w1:p2".into(),
            workspace: Some("~".into()),
            cwd: Some("/tmp/proj".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let broken = "xcrun notarytool store-credentials copypasta --apple-id a@b.c --team-id\n  ABCDEF1234";
    let (a, _) = s.record_copy(&text(broken), &claude, 1_000).unwrap();
    s.record_copy(&text(broken), &claude, 1_060).unwrap();
    s.record_paste(a, &shell, 10_000).unwrap();
    s.record_paste(a, &shell, 10_000).unwrap();
    s.record_paste(a, &shell, 22_000).unwrap();
    let out = format!("{broken}\nError: Missing value for '--team-id <team-id>'\nzsh: command not found: ABCDEF1234");
    let (b, _) = s.record_copy(&text(&out), &shell, 26_000).unwrap();
    s.record_paste(b, &claude, 37_000).unwrap();
    let fixed = "xcrun notarytool store-credentials copypasta --apple-id a@b.c --team-id ABCDEF1234";
    let (c, _) = s.record_copy(&text(fixed), &claude, 55_000).unwrap();

    let ca = s.clip(a).unwrap().unwrap();
    assert_eq!((ca.copies, ca.pastes), (1, 2), "repeats within a second are one action");

    let g = graph::build(&s, c).unwrap();
    let ids: Vec<i64> = g.clips.iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![a, b, c], "oldest first");
    let first = &g.clips[0];
    assert_eq!(first.steps.len(), 2, "copy, then pastes into one place collapsed: {:?}", first.steps);
    assert_eq!(first.steps[1].count, 2);
    assert_eq!(first.steps[1].title, "shell");
    let rel: Vec<(i64, &str)> = g.relations.iter().map(|r| (r.child, r.text.as_str())).collect();
    assert!(rel.contains(&(b, &*format!("includes #{a}"))), "{rel:?}");
    assert!(rel.iter().any(|(ch, t)| *ch == c && t.contains(&format!("#{a}"))), "{rel:?}");
    let txt = graph::render_text(&g);
    assert!(txt.contains("◀ this clip") && txt.contains("×2") && txt.contains("↳ includes"), "{txt}");
}
