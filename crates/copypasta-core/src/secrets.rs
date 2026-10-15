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
    PATTERNS.iter().any(|re| re.is_match(text)) || looks_like_password(text)
}

/// A lone generated-password-looking token: no spaces, 10–64 chars, upper and
/// lower case, a digit and a symbol, and not a URL, path or email address.
pub fn looks_like_password(text: &str) -> bool {
    let t = text.trim();
    let n = t.chars().count();
    if !(10..=64).contains(&n) || t.chars().any(char::is_whitespace) {
        return false;
    }
    if t.contains("://") || t.contains('/') || t.contains('\\') || (t.contains('@') && t.contains('.') && !t.ends_with('@')) {
        return false;
    }
    let upper = t.chars().any(|c| c.is_ascii_uppercase());
    let lower = t.chars().any(|c| c.is_ascii_lowercase());
    let digit = t.chars().any(|c| c.is_ascii_digit());
    let symbol = t.chars().any(|c| c.is_ascii_punctuation());
    upper && lower && digit && symbol
}

/// Password managers that don't mark their clipboard writes as concealed.
/// Nothing copied from them is recorded.
pub const PASSWORD_MANAGERS: &[&str] = &[
    "com.bitwarden.desktop",
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.agilebits.onepassword-osx",
    "org.keepassxc.keepassxc",
    "com.apple.Passwords",
    "com.dashlane.dashlanephonefinal",
    "in.sinew.Enpass-Desktop",
    "com.lastpass.LastPass",
    "com.markmcguill.strongbox.mac",
    "com.proton.pass",
];

pub fn is_password_manager(bundle_id: Option<&str>) -> bool {
    bundle_id.is_some_and(|b| PASSWORD_MANAGERS.iter().any(|p| p.eq_ignore_ascii_case(b)))
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
        assert!(super::looks_secret("Xk2#mPq9!vRt"));
        assert!(super::looks_secret("Xk2#mPq9!vRt@"));
        assert!(!super::looks_secret("https://Example.com/a-1"));
        assert!(!super::looks_secret("~/workspaces/Copy-1"));
        assert!(!super::looks_secret("Jane.Doe1@example.com"));
        assert!(!super::looks_secret("copypasta"));
        assert!(!super::looks_secret("YW2LDT8Y6K"));
        assert!(super::is_password_manager(Some("com.bitwarden.desktop")));
    }
}
