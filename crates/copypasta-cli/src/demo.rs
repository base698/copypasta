//! `copypasta demo`: seed a realistic history (into $COPYPASTA_HOME) for
//! screenshots and UI work.

use anyhow::Result;
use copypasta_core::context::{HerdrCtx, SlackCtx};
use copypasta_core::{imaging, Captured, ClipKind, Context, Store};

fn web(url: &str, title: &str) -> Context {
    Context { app_name: Some("Google Chrome".into()), bundle_id: Some("com.google.Chrome".into()), url: Some(url.into()), page_title: Some(title.into()), ..Default::default() }
}
fn slack(chan: &str, id: &str) -> Context {
    Context {
        app_name: Some("Google Chrome".into()),
        url: Some(format!("https://app.slack.com/client/T0ACME/{id}")),
        page_title: Some(format!("{chan} (Channel) - Acme - Slack")),
        slack: Some(SlackCtx { conversation: format!("#{chan}"), workspace: Some("Acme".into()), channel_id: Some(id.into()) }),
        ..Default::default()
    }
}
fn herdr(ws: &str, pane: &str, agent: Option<&str>, tab: &str, cwd: &str, title: &str) -> Context {
    Context {
        app_name: Some("Ghostty".into()),
        bundle_id: Some("com.mitchellh.ghostty".into()),
        herdr: Some(HerdrCtx {
            pane_id: pane.into(),
            workspace: Some(ws.into()),
            tab: Some(tab.into()),
            agent: agent.map(Into::into),
            agent_session: agent.map(|_| "3f2a9c1e-77b0-4d51-9a1e-1b2c3d4e5f60".into()),
            cwd: Some(cwd.into()),
            title: Some(title.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn text(t: &str) -> Captured {
    let kind = if t.starts_with("http") && !t.contains(' ') { ClipKind::Url } else { ClipKind::Text };
    Captured { kind, text: Some(t.into()), ..Default::default() }
}

fn gradient_png(w: u32, h: u32, seed: u8) -> Captured {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x * 255 / w) as u8 ^ seed, (y * 255 / h) as u8, 160u8.wrapping_add(seed)])
    });
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut buf, image::ImageFormat::Png).unwrap();
    let n = imaging::to_png(buf.get_ref()).unwrap();
    Captured { kind: ClipKind::Image, png: Some(n.png), width: Some(n.width), height: Some(n.height), ..Default::default() }
}

pub fn seed() -> Result<()> {
    if std::env::var_os("COPYPASTA_HOME").filter(|v| !v.is_empty()).is_none() {
        anyhow::bail!("refusing to seed your real history; set COPYPASTA_HOME=/some/scratch/dir");
    }
    let mut s = Store::open()?;
    let now = copypasta_core::now_ms();
    let m = |mins: i64| now - mins * 60_000;
    let claude = herdr("copypasta", "w2:p4", Some("claude"), "agent", "/Users/me/workspaces/copypasta", "Fix event tap re-enable");
    let codex = herdr("api", "w1:p7", Some("codex"), "agent", "/Users/me/workspaces/api", "Rate limiter");
    let shell = herdr("api", "w1:p2", None, "logs", "/Users/me/workspaces/api", "tail -f api.log");

    // 1. Error from logs → Claude → fix copied out → Slack #eng
    let err = "thread 'tokio-runtime-worker' panicked at src/limiter.rs:88:14:\ncalled `Option::unwrap()` on a `None` value";
    let (e, _) = s.record_copy(&text(err), &shell, m(95))?;
    s.record_paste(e, &codex, m(94))?;
    let fix = "let bucket = self.buckets.get(&key).ok_or(LimitError::UnknownKey(key.clone()))?;\n// was: self.buckets.get(&key).unwrap()";
    let (f, _) = s.record_copy(&text(fix), &codex, m(88))?;
    s.record_paste(f, &herdr("api", "w1:p5", None, "editor", "/Users/me/workspaces/api", "nvim src/limiter.rs"), m(87))?;
    s.record_paste(f, &slack("eng", "C02ENG"), m(80))?;
    let (f2, _) = s.record_copy(&text("self.buckets.get(&key).ok_or(LimitError::UnknownKey(key.clone()))?"), &slack("eng", "C02ENG"), m(41))?;
    s.record_paste(f2, &web("https://github.com/acme/api/pull/412", "Fix limiter panic by acme · Pull Request #412"), m(40))?;

    // 2. Docs URL → Claude → answer → Slack
    let (u, _) = s.record_copy(&text("https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html"), &web("https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html", "spawn_blocking in tokio::task - Rust"), m(70))?;
    s.record_paste(u, &claude, m(69))?;
    let ans = "spawn_blocking moves the closure onto tokio's blocking pool, so the CGEventTap run loop never stalls an async worker. Keep the tap on its own OS thread, though: CFRunLoopRun() never returns.";
    let (a, _) = s.record_copy(&text(ans), &claude, m(60))?;
    s.record_paste(a, &slack("copypasta", "C09CP"), m(58))?;
    // ...and handed to codex in another workspace: incoming there.
    s.record_paste(a, &codex, m(57))?;

    // 3. Screenshot region → Slack and Claude; same image re-copied from Slack
    let shot = gradient_png(640, 360, 0);
    let mut sc = Context { note: Some("Screenshot".into()), app_name: Some("Figma".into()), ..Default::default() };
    sc.window_title = Some("Panel v2".into());
    let (i, _) = s.record_copy(&shot, &sc, m(30))?;
    s.record_paste(i, &slack("design", "C03DSN"), m(29))?;
    s.record_paste(i, &claude, m(25))?;
    let (_i2, _) = s.record_copy(&gradient_png(640, 360, 1), &slack("design", "C03DSN"), m(12))?;

    // 4. Some plain history
    let (g, _) = s.record_copy(&text("cargo build --release -p copypasta-app && ./scripts/install-macos.sh"), &claude, m(9))?;
    s.record_paste(g, &shell, m(8))?;
    s.record_copy(&text("https://github.com/base698/copypasta"), &web("https://github.com/base698/copypasta", "base698/copypasta"), m(3))?;
    s.record_copy(&text("ANTHROPIC_API_KEY=sk-ant-api03-not-a-real-key-0000000000000000"), &shell, m(2))?;
    // 5. A hand-made group, and enough history in api to page through.
    let notes = s.create_group("Release notes")?;
    let (n1, _) = s.record_copy(&text("Groups: clips collapse under their herdr workspace; drag to regroup."), &claude, m(7))?;
    let (n2, _) = s.record_copy(&text("Paging: the list loads 40 clips per group as you scroll."), &claude, m(6))?;
    s.move_clips(&[n1, n2], Some(notes))?;
    for i in 0..70 {
        s.record_copy(
            &text(&format!("2026-10-15T09:{:02}:{:02}Z INFO api::limiter bucket={} tokens={}", i / 2, (i * 7) % 60, i % 9, 100 - i)),
            &shell,
            m(200 + i),
        )?;
    }
    println!("seeded demo history into {}", copypasta_core::paths::data_dir().display());
    Ok(())
}
