//! Clipboard text that looks like a credential is never stored in clear:
//! we keep its hash (so lineage still works) and a masked preview.

use once_cell::sync::Lazy;
use regex::Regex;

static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"sk-ant-[A-Za-z0-9_\-]{20,}",
        r"sk-(proj-)?[A-Za-z0-9_\-]{20,}",
        r"AKIA[0-9A-Z]{16}",
        r"gh[pousr]_[A-Za-z0-9]{30,}",
        r"github_pat_[A-Za-z0-9_]{30,}",
        r"xox[abprs]-[A-Za-z0-9\-]{10,}",
        r"AIza[0-9A-Za-z_\-]{35}",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        r"eyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
        r"(?i)(password|passwd|secret|api[_-]?key|token)\s*[:=]\s*\S{8,}",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

pub fn looks_secret(text: &str) -> bool {
    PATTERNS.iter().any(|re| re.is_match(text))
}

pub fn mask(text: &str) -> String {
    let t = text.trim();
    let head: String = t.chars().take(6).collect();
    format!("{head}… (secret redacted, {} chars)", t.chars().count())
}

#[cfg(test)]
mod tests {
    #[test]
    fn detects() {
        assert!(super::looks_secret("export KEY=sk-ant-api03-aaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(super::looks_secret("AKIAABCDEFGHIJKLMNOP"));
        assert!(!super::looks_secret("just some ordinary text about tokens"));
    }
}
