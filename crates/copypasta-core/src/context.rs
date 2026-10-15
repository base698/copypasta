//! Where a copy came from or a paste went: the app/window plus richer
//! details for browsers, Slack and herdr panes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Context {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slack: Option<SlackCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub herdr: Option<HerdrCtx>,
    /// Set for synthetic sources such as region screenshots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SlackCtx {
    pub conversation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HerdrCtx {
    pub pane_id: String,
    /// Workspace label (what groups are keyed on).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Which family a context belongs to; drives icons and colors in the UIs.
pub fn kind(ctx: &Context) -> &'static str {
    if ctx.herdr.is_some() {
        "herdr"
    } else if ctx.slack.is_some() {
        "slack"
    } else if ctx.url.is_some() {
        "web"
    } else if ctx.note.is_some() {
        "system"
    } else {
        "app"
    }
}

impl Context {
    /// Fill in derived fields (currently Slack) from the URL and titles.
    pub fn enrich(mut self) -> Self {
        if self.slack.is_none() {
            self.slack = parse_slack(self.url.as_deref(), self.title_for_parsing());
        }
        self
    }

    /// Places that answer what you paste into them (a herdr pane running an
    /// agent, or a web chat assistant), so a later copy out of them is
    /// probably derived from the paste.
    pub fn is_assistant(&self) -> bool {
        if let Some(h) = &self.herdr {
            return h.agent.is_some();
        }
        const HOSTS: &[&str] = &["claude.ai", "chatgpt.com", "chat.openai.com", "gemini.google.com", "perplexity.ai", "copilot.microsoft.com"];
        self.url
            .as_deref()
            .and_then(url_host)
            .map(|h| HOSTS.iter().any(|x| h == *x || h.ends_with(&format!(".{x}"))))
            .unwrap_or(false)
    }

    fn title_for_parsing(&self) -> Option<&str> {
        self.page_title.as_deref().or(self.window_title.as_deref())
    }

    /// Stable identity of the place, used to connect "pasted into X" with a
    /// later "copied from X".
    pub fn key(&self) -> String {
        if let Some(h) = &self.herdr {
            return format!("herdr:{}", h.pane_id);
        }
        if let Some(s) = &self.slack {
            let ws = s.workspace.as_deref().unwrap_or("");
            let conv = s.channel_id.as_deref().unwrap_or(&s.conversation);
            return format!("slack:{ws}/{conv}");
        }
        if let Some(u) = &self.url {
            return format!("web:{}", normalize_url(u));
        }
        if let Some(n) = &self.note {
            return format!("system:{n}");
        }
        format!(
            "app:{}/{}",
            self.bundle_id.as_deref().or(self.app_name.as_deref()).unwrap_or("?"),
            self.window_title.as_deref().unwrap_or("")
        )
    }

    /// One-line human label: "Slack #general", "herdr claude · ~/workspaces", ...
    pub fn label(&self) -> String {
        if let Some(h) = &self.herdr {
            let who = h
                .agent
                .clone()
                .or_else(|| h.pane_label.clone())
                .unwrap_or_else(|| "shell".into());
            let mut s = format!("herdr {who}");
            if let Some(t) = h.tab.as_ref().filter(|t| !t.chars().all(|c| c.is_ascii_digit())) {
                s.push_str(&format!(" · {t}"));
            }
            if let Some(cwd) = &h.cwd {
                s.push_str(&format!(" · {}", tilde(cwd)));
            }
            return s;
        }
        if let Some(sl) = &self.slack {
            let conv = if sl.conversation.starts_with('#') || sl.conversation.starts_with('@') {
                sl.conversation.clone()
            } else {
                format!("#{}", sl.conversation)
            };
            return match &sl.workspace {
                Some(ws) => format!("Slack {conv} ({ws})"),
                None => format!("Slack {conv}"),
            };
        }
        if let Some(u) = &self.url {
            let host = url_host(u).unwrap_or_else(|| u.clone());
            return match self.title_for_parsing() {
                Some(t) if !t.is_empty() => format!("{host} — {}", truncate(t, 60)),
                _ => host,
            };
        }
        if let Some(n) = &self.note {
            return match &self.app_name {
                Some(a) => format!("{n} · over {a}"),
                None => n.clone(),
            };
        }
        let app = self.app_name.clone().unwrap_or_else(|| "Unknown app".into());
        match &self.window_title {
            Some(t) if !t.is_empty() && t != &app => format!("{app} — {}", truncate(t, 60)),
            _ => app,
        }
    }
}

pub fn tilde(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

pub fn url_host(u: &str) -> Option<String> {
    let rest = u.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    Some(host.trim_start_matches("www.").to_string())
}

/// Drop fragments and tracking params so the same page maps to one key.
pub fn normalize_url(u: &str) -> String {
    let no_frag = u.split('#').next().unwrap_or(u);
    match no_frag.split_once('?') {
        Some((base, q)) => {
            let kept: Vec<&str> = q
                .split('&')
                .filter(|p| {
                    let k = p.split('=').next().unwrap_or("");
                    !(k.starts_with("utm_") || k == "fbclid" || k == "gclid")
                })
                .collect();
            if kept.is_empty() {
                base.to_string()
            } else {
                format!("{base}?{}", kept.join("&"))
            }
        }
        None => no_frag.to_string(),
    }
}

/// Slack in a browser: app.slack.com/client/<team>/<channel> plus a tab title
/// such as "general (Channel) - Acme - Slack" or "* Jane Doe (DM) - Acme - Slack".
pub fn parse_slack(url: Option<&str>, title: Option<&str>) -> Option<SlackCtx> {
    let from_url = url.map(|u| u.contains("app.slack.com/") || u.contains(".slack.com/")).unwrap_or(false);
    let title = title.map(str::trim);
    let from_title = title.map(|t| t.ends_with(" - Slack") || t == "Slack").unwrap_or(false);
    if !from_url && !from_title {
        return None;
    }
    let channel_id = url.and_then(|u| {
        let path = u.split("/client/").nth(1)?;
        let mut parts = path.split(['/', '?', '#']);
        let _team = parts.next()?;
        parts.next().filter(|c| !c.is_empty()).map(str::to_string)
    });
    let (mut conversation, mut workspace) = (None, None);
    if let Some(t) = title {
        let t = t.trim_end_matches(" - Slack");
        let t = t.trim_start_matches(['*', '!', ' ']);
        let parts: Vec<&str> = t.split(" - ").collect();
        if parts.len() >= 2 {
            workspace = Some(parts[parts.len() - 1].trim().to_string());
            conversation = Some(parts[..parts.len() - 1].join(" - "));
        } else if !t.is_empty() && t != "Slack" {
            conversation = Some(t.to_string());
        }
    }
    let conversation = conversation.map(|c| {
        let c = c.trim();
        for (suffix, prefix) in [(" (Channel)", "#"), (" (Private)", "#"), (" (DM)", "@")] {
            if let Some(name) = c.strip_suffix(suffix) {
                return format!("{prefix}{name}");
            }
        }
        c.to_string()
    });
    Some(SlackCtx {
        conversation: conversation
            .or_else(|| channel_id.clone())
            .unwrap_or_else(|| "slack".into()),
        workspace,
        channel_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_title_parsing() {
        let s = parse_slack(
            Some("https://app.slack.com/client/T123/C456"),
            Some("general (Channel) - Acme - Slack"),
        )
        .unwrap();
        assert_eq!(s.conversation, "#general");
        assert_eq!(s.workspace.as_deref(), Some("Acme"));
        assert_eq!(s.channel_id.as_deref(), Some("C456"));
        let dm = parse_slack(None, Some("* Jane Doe (DM) - Acme - Slack")).unwrap();
        assert_eq!(dm.conversation, "@Jane Doe");
        assert!(parse_slack(Some("https://example.com"), Some("Example")).is_none());
    }

    #[test]
    fn keys_and_labels() {
        let ctx = Context {
            url: Some("https://www.example.com/a?utm_source=x&id=2#frag".into()),
            page_title: Some("Example page".into()),
            ..Default::default()
        };
        assert_eq!(ctx.key(), "web:https://www.example.com/a?id=2");
        assert_eq!(ctx.label(), "example.com — Example page");
    }
}
