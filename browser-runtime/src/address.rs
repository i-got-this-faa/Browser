//! Address resolution: turn whatever a person typed (or a script passed)
//! into the URL the engine should load. The only place that decides
//! "URL or search"; every entry point goes through [`resolve`].
//!
//! Rules, in order:
//! 1. Blank input resolves to the empty string (callers treat it as "nothing").
//! 2. A known scheme (`http://`, `https://`, `file://`, `chrome://`,
//!    `about:`, `data:`, `view-source:`, and the shell's own `strip://`)
//!    passes through unchanged.
//! 3. Other text containing whitespace is a search.
//! 4. Otherwise it is host-like when the part before the first `/ ? #` is
//!    - `localhost` (or `*.localhost`), an IPv4 address, or a bracketed IPv6
//!      address, each with an optional `:port`; or
//!    - any hostname with an explicit `:port`; or
//!    - a domain: two or more dot-separated labels whose last label (the TLD)
//!      is two or more letters.
//!
//!    Host-like input gets `http://` for localhost and IP addresses, and
//!    `https://` for everything else.
//! 5. Anything else (`youtube`, `rust gpui`, `1.5`, `me@example.com`) is a
//!    search on the configured engine.
//!
//! There is deliberately no TLD list: it would go stale (new TLDs ship
//! yearly) and misroute the ones it misses. "Dot plus alphabetic TLD"
//! errs toward navigating, so `main.rs` and `notes.txt` load as hosts.

use std::net::{Ipv4Addr, Ipv6Addr};

/// Schemes that are already complete addresses.
const PASSTHROUGH_SCHEMES: &[&str] =
    &["http://", "https://", "file://", "chrome://", "about:", "data:", "view-source:", "strip://"];

/// Resolve typed text to a URL, or to a search URL built from
/// `search_engine_url` (`{}` is replaced by the percent-encoded query).
pub fn resolve(input: &str, search_engine_url: &str) -> String {
    let input = input.trim();
    if input.is_empty() {
        return String::new();
    }
    match classify(input) {
        Address::Url(url) => url,
        Address::Search(query) => search_engine_url.replacen("{}", &percent_encode(&query), 1),
    }
}

/// What a piece of typed text means.
#[derive(Debug, PartialEq, Eq)]
enum Address {
    Url(String),
    Search(String),
}

fn classify(input: &str) -> Address {
    let lower = input.to_ascii_lowercase();
    if PASSTHROUGH_SCHEMES.iter().any(|s| lower.starts_with(s)) {
        return Address::Url(input.to_string());
    }
    if input.chars().any(char::is_whitespace) {
        return Address::Search(input.to_string());
    }
    let authority = input.split(['/', '?', '#']).next().unwrap_or(input);
    match host_kind(authority) {
        Some(HostKind::Local) => Address::Url(format!("http://{input}")),
        Some(HostKind::Remote) => Address::Url(format!("https://{input}")),
        None => Address::Search(input.to_string()),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum HostKind {
    /// localhost or an IP literal: served over plain http.
    Local,
    /// A named host: https.
    Remote,
}

/// Classify `host[:port]` / `[v6][:port]`; None when it is not host-like.
fn host_kind(authority: &str) -> Option<HostKind> {
    let (host, has_port) = split_port(authority)?;
    if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return v6.parse::<Ipv6Addr>().ok().map(|_| HostKind::Local);
    }
    let host = host.to_ascii_lowercase();
    if !is_hostname(&host) {
        return None;
    }
    if host == "localhost" || host.ends_with(".localhost") || host.parse::<Ipv4Addr>().is_ok() {
        return Some(HostKind::Local);
    }
    (has_port || is_domain(&host)).then_some(HostKind::Remote)
}

/// Split an optional trailing `:port` (1-65535) off the authority. None when
/// a colon is present but what follows is not a valid port.
fn split_port(authority: &str) -> Option<(&str, bool)> {
    let colon = match authority.strip_prefix('[') {
        // `[::1]:8080`: the port colon is the one right after `]`.
        Some(rest) => rest
            .find(']')
            .map(|i| i + 2) // index of the char after `]`
            .filter(|&i| authority[i..].starts_with(':')),
        None => authority.rfind(':'),
    };
    let Some(i) = colon else { return Some((authority, false)) };
    let (host, port) = (&authority[..i], &authority[i + 1..]);
    let valid = port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p > 0);
    valid.then_some((host, true))
}

/// Every label is non-empty letters/digits/hyphens (no leading or trailing
/// hyphen, at most 63 chars).
fn is_hostname(host: &str) -> bool {
    host.split('.').all(|l| {
        !l.is_empty()
            && l.chars().count() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_alphanumeric() || c == '-')
    })
}

/// A hostname with a dot whose last label looks like a TLD.
fn is_domain(host: &str) -> bool {
    let Some((_, tld)) = host.rsplit_once('.') else { return false };
    tld.starts_with("xn--") || (tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic))
}

/// Percent-encode a query for use inside a URL (RFC 3986 unreserved kept).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGINE: &str = "https://search.test/?q={}";

    fn r(input: &str) -> String {
        resolve(input, ENGINE)
    }

    #[test]
    fn shell_pages_pass_through() {
        assert_eq!(r("strip://settings"), "strip://settings");
    }

    #[test]
    fn domains_get_https() {
        assert_eq!(r("google.com"), "https://google.com");
        assert_eq!(r("example.org/path"), "https://example.org/path");
        assert_eq!(r("www.example.co.uk/a?b=1#c"), "https://www.example.co.uk/a?b=1#c");
        assert_eq!(r("  example.com  "), "https://example.com");
        assert_eq!(r("Example.COM"), "https://Example.COM");
        assert_eq!(r("example.com:8443/x"), "https://example.com:8443/x");
        assert_eq!(r("bücher.de"), "https://bücher.de");
        assert_eq!(r("xn--bcher-kva.example"), "https://xn--bcher-kva.example");
    }

    #[test]
    fn localhost_and_ips_get_http() {
        assert_eq!(r("localhost"), "http://localhost");
        assert_eq!(r("localhost:3000"), "http://localhost:3000");
        assert_eq!(r("LOCALHOST:3000/app"), "http://LOCALHOST:3000/app");
        assert_eq!(r("app.localhost:3000"), "http://app.localhost:3000");
        assert_eq!(r("127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(r("192.168.1.1/admin"), "http://192.168.1.1/admin");
        assert_eq!(r("[::1]"), "http://[::1]");
        assert_eq!(r("[::1]:8080/x"), "http://[::1]:8080/x");
        assert_eq!(r("[2001:db8::1]"), "http://[2001:db8::1]");
    }

    #[test]
    fn a_port_alone_makes_a_host() {
        assert_eq!(r("myserver:3000"), "https://myserver:3000");
    }

    #[test]
    fn full_urls_pass_through_unchanged() {
        for u in [
            "https://example.com/a b",
            "http://x.test",
            "HTTPS://X.TEST",
            "file:///home/me/a.html",
            "about:blank",
            "data:text/html,<h1>hi</h1>",
            "chrome://version",
            "view-source:https://example.com",
        ] {
            assert_eq!(r(u), u, "{u}");
        }
    }

    #[test]
    fn everything_else_is_a_search() {
        assert_eq!(r("youtube"), "https://search.test/?q=youtube");
        assert_eq!(r("rust gpui"), "https://search.test/?q=rust%20gpui");
        assert_eq!(r("what is 1.5 + 2"), "https://search.test/?q=what%20is%201.5%20%2B%202");
        assert_eq!(r("1.5"), "https://search.test/?q=1.5");
        assert_eq!(r("192.168.1"), "https://search.test/?q=192.168.1");
        assert_eq!(r("me@example.com"), "https://search.test/?q=me%40example.com");
        assert_eq!(r("site:example.com"), "https://search.test/?q=site%3Aexample.com");
        assert_eq!(r("rust/gpui"), "https://search.test/?q=rust%2Fgpui");
        assert_eq!(r("e.g."), "https://search.test/?q=e.g.");
        assert_eq!(r("::1"), "https://search.test/?q=%3A%3A1");
        assert_eq!(r("localhost:abc"), "https://search.test/?q=localhost%3Aabc");
        assert_eq!(r("host:99999"), "https://search.test/?q=host%3A99999");
        assert_eq!(r("-bad-.com"), "https://search.test/?q=-bad-.com");
        assert_eq!(r("çay"), "https://search.test/?q=%C3%A7ay");
    }

    #[test]
    fn blank_resolves_to_nothing() {
        assert_eq!(r(""), "");
        assert_eq!(r("   \t"), "");
    }

    #[test]
    fn resolving_a_result_is_idempotent() {
        for input in ["google.com", "youtube", "rust gpui", "localhost:3000", "[::1]"] {
            let once = r(input);
            assert_eq!(r(&once), once, "{input}");
        }
    }
}
