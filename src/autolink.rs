//! URL detection for paste-to-link handling.
//!
//! The paste handler consults [`looks_like_url`] to decide whether a pasted
//! fragment should become a `link` node instead of plain text, and
//! [`normalize_href`] to derive a navigable destination from a bare host.

/// True when `s` (after trimming) is a single token that reads as a web URL:
/// an explicit `http(s)://…`, or a bare `www.…`. Anything containing
/// whitespace, or a bare host without a scheme/`www.` prefix, is rejected so
/// ordinary prose like `file.rs` or `see foo.bar` is never autolinked.
#[cfg(any(target_arch = "wasm32", test))]
pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    let (rest, explicit_scheme) = if let Some(r) = lower.strip_prefix("http://") {
        (r, true)
    } else if let Some(r) = lower.strip_prefix("https://") {
        (r, true)
    } else if lower.starts_with("www.") {
        (lower.as_str(), false)
    } else {
        return false;
    };
    // Isolate the authority, then strip userinfo and port to reach the host.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let hostname = authority.split(':').next().unwrap_or(authority);
    if hostname.is_empty() {
        return false;
    }
    let labels: Vec<&str> = hostname.split('.').collect();
    if labels.iter().any(|l| l.is_empty() || !is_host_label(l)) {
        return false;
    }
    // With a scheme a single-label host (`localhost`, an intranet name) is
    // legitimate; a bare `www.` form needs at least one label after `www`.
    if explicit_scheme {
        true
    } else {
        labels.len() >= 2
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn is_host_label(l: &str) -> bool {
    l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Turn a pasted URL token into a navigable href, prefixing `https://` when
/// no scheme is present (the bare `www.` case).
pub fn normalize_href(s: &str) -> String {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        t.to_string()
    } else {
        format!("https://{t}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_explicit_urls() {
        assert!(looks_like_url("https://example.com"));
        assert!(looks_like_url("http://example.com/path?q=1#frag"));
        assert!(looks_like_url("https://sub.example.co.uk:8443/a"));
        assert!(looks_like_url("  https://example.com  "));
        assert!(looks_like_url("http://localhost:8080"));
        assert!(looks_like_url("www.example.com"));
        assert!(looks_like_url("www.example.com/path"));
    }

    #[test]
    fn rejects_non_urls() {
        assert!(!looks_like_url(""));
        assert!(!looks_like_url("hello world"));
        assert!(!looks_like_url("example.com")); // bare host, no scheme/www
        assert!(!looks_like_url("file.rs"));
        assert!(!looks_like_url("https://")); // empty host
        assert!(!looks_like_url("ftp://example.com")); // unsupported scheme
        assert!(!looks_like_url("https://example.com extra")); // trailing text
        assert!(!looks_like_url("www."));
    }

    #[test]
    fn normalizes_scheme() {
        assert_eq!(normalize_href("https://x.com"), "https://x.com");
        assert_eq!(normalize_href("HTTP://x.com"), "HTTP://x.com");
        assert_eq!(normalize_href("www.x.com"), "https://www.x.com");
        assert_eq!(normalize_href("  www.x.com "), "https://www.x.com");
    }
}
