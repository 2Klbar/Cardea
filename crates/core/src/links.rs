//! Links (security R3, R4): the checks on URLs that come in from the host for a sandbox app, and on those a
//! sandbox sends out for the host browser, with the marks that make an outgoing link suspicious.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

/// Longest URL accepted, either way.
pub const MAX_URL: usize = 4096;
/// Longer than this, an outgoing link is suspicious.
pub const LONG_URL: usize = 2000;
/// Schemes an app can never claim: the web, files and the browser's own.
pub const RESERVED: [&str; 14] = [
    "http",
    "https",
    "file",
    "ftp",
    "ftps",
    "ws",
    "wss",
    "data",
    "javascript",
    "about",
    "blob",
    "view-source",
    "resource",
    "chrome",
];
/// The guest opener, installed as `xdg-open` (and `BROWSER`) in a sandbox whose links may leave.
pub const OPENER: &str = include_str!("../guest/xdg-open.sh");
/// Where the link FIFO is mounted in the sandbox.
pub const FIFO_MOUNT: &str = "/run/cardea/links";
/// Where the opener is mounted in the sandbox.
pub const OPENER_MOUNT: &str = "/usr/local/bin/xdg-open";

/// A scheme an app declares: `[a-z][a-z0-9+.-]*`, at most 32 characters, not reserved.
pub fn scheme(s: &str) -> Result<(), String> {
    let mut chars = s.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c));
    if !valid || s.len() > 32 {
        return Err(format!(
            "{s:?} is not a valid scheme: use 1 to 32 lowercase letters, digits, '+', '.' or '-', starting with a \
             letter"
        ));
    }
    if RESERVED.contains(&s) {
        return Err(format!(
            "{s:?} belongs to the browser: an app cannot claim it"
        ));
    }
    Ok(())
}

/// An allowed host: a lowercase ASCII domain (IDN as punycode), no port, no IP address.
pub fn host(s: &str) -> Result<(), String> {
    match normalize_host(s) {
        Ok(n) if n == s => Ok(()),
        Ok(n) => Err(format!("write {s:?} as {n:?}")),
        Err(e) => Err(e),
    }
}

/// The stored form of a host typed by the owner: lowercase, IDN as punycode.
pub fn normalize_host(s: &str) -> Result<String, String> {
    let bad = || format!("{s:?} is not a domain name such as github.com");
    let t = s.trim();
    if t.is_empty() || t.contains([':', '/', '@', '[', ']']) || t.ends_with('.') {
        return Err(bad());
    }
    match Host::parse(t).map_err(|_| bad())? {
        Host::Domain(d)
            if d.split('.').all(|l| {
                !l.is_empty()
                    && l.len() <= 63
                    && !l.starts_with('-')
                    && !l.ends_with('-')
                    && l.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            }) && d.len() <= 253 =>
        {
            Ok(d)
        }
        Host::Domain(_) => Err(bad()),
        Host::Ipv4(_) | Host::Ipv6(_) => Err(format!(
            "{s:?} is an IP address: only domain names can be allowed"
        )),
    }
}

/// No byte below space, nor DEL, nor any other control character.
fn clean(s: &str) -> Result<(), String> {
    if s.len() > MAX_URL {
        return Err(format!("the link is longer than {MAX_URL} bytes"));
    }
    if s.chars().any(char::is_control) {
        return Err("the link has a control character".into());
    }
    Ok(())
}

/// An incoming link for an app (R4): starts with one of its schemes, at most 4 KiB, no control character, parses.
/// Returned unchanged: it becomes the app's last argument.
pub fn incoming<'a>(url: &'a str, schemes: &[String]) -> Result<&'a str, String> {
    clean(url)?;
    let lower = url.to_ascii_lowercase();
    if !schemes.iter().any(|s| lower.starts_with(&format!("{s}:"))) {
        return Err(format!(
            "this app opens only {} links",
            schemes
                .iter()
                .map(|s| format!("{s}:"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Url::parse(url).map_err(|e| format!("not a valid link: {e}"))?;
    Ok(url)
}

/// What makes an outgoing link suspicious; each comes with a plain sentence for the owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mark", content = "value", rename_all = "snake_case")]
pub enum Mark {
    /// A name before the domain (`a@b`): the value is the real domain.
    UserInfo(String),
    /// A non-Latin domain that may imitate another one: its punycode form.
    Idn(String),
    IpLiteral,
    /// Your computer or local network.
    Local,
    Port(u16),
    Long,
}

impl Mark {
    /// The sentence the link window shows: text, the part in bold, text.
    pub fn sentence(&self) -> (String, String, String) {
        let s = |a: &str, b: String, c: &str| (a.to_owned(), b, c.to_owned());
        match self {
            Mark::UserInfo(d) => s(
                "The address has a name before the domain: the real domain is ",
                d.clone(),
                ".",
            ),
            Mark::Idn(p) => s(
                "The domain uses non-Latin letters and may imitate another one: ",
                p.clone(),
                ".",
            ),
            Mark::IpLiteral => s(
                "The link goes to a bare IP address, not a domain.",
                String::new(),
                "",
            ),
            Mark::Local => s(
                "The link goes to your computer or local network.",
                String::new(),
                "",
            ),
            Mark::Port(p) => s("The link uses the unusual port ", p.to_string(), "."),
            Mark::Long => s(
                "The address is very long; data may be hidden in it.",
                String::new(),
                "",
            ),
        }
    }

    pub fn text(&self) -> String {
        let (a, b, c) = self.sentence();
        format!("{a}{b}{c}")
    }
}

/// An outgoing link, checked and serialised again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outgoing {
    /// The link as the parser writes it back: the only string that goes on.
    pub url: String,
    /// Its host (punycode for an IDN, without brackets for IPv6).
    pub host: String,
    pub marks: Vec<Mark>,
}

/// Checks a link written by a sandbox (R3): http or https only, parsed (WHATWG) and serialised again.
pub fn outgoing(line: &str) -> Result<Outgoing, String> {
    clean(line)?;
    let url = Url::parse(line).map_err(|e| format!("not a valid link: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "{}: links are refused, only http and https leave a sandbox",
            url.scheme()
        ));
    }
    let host = url.host().ok_or("the link has no host")?;
    let mut marks = Vec::new();
    let name = match &host {
        Host::Domain(d) => (*d).to_owned(),
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => ip.to_string(),
    };
    if !url.username().is_empty() || url.password().is_some() {
        marks.push(Mark::UserInfo(name.clone()));
    }
    match &host {
        Host::Domain(d) => {
            if d.split('.').any(|l| l.starts_with("xn--")) {
                marks.push(Mark::Idn(name.clone()));
            }
            if *d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || !d.contains('.')
            {
                marks.push(Mark::Local);
            }
        }
        Host::Ipv4(ip) => {
            marks.push(Mark::IpLiteral);
            if local_ip(IpAddr::V4(*ip)) {
                marks.push(Mark::Local);
            }
        }
        Host::Ipv6(ip) => {
            marks.push(Mark::IpLiteral);
            if local_ip(IpAddr::V6(*ip)) {
                marks.push(Mark::Local);
            }
        }
    }
    // `port()` is `None` for the scheme's default port.
    if let Some(p) = url.port() {
        marks.push(Mark::Port(p));
    }
    if url.as_str().len() > LONG_URL {
        marks.push(Mark::Long);
    }
    Ok(Outgoing {
        url: url.as_str().to_owned(),
        host: name,
        marks,
    })
}

fn local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.octets()[0] == 100 && v4.octets()[1] & 0xc0 == 64
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| local_ip(IpAddr::V4(v4)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(url: &str) -> Vec<Mark> {
        outgoing(url).unwrap().marks
    }

    #[test]
    fn schemes() {
        for ok in ["claude", "web+app", "x-y.z", "a1"] {
            assert!(scheme(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Claude",
            "1abc",
            "a b",
            "a:",
            "https",
            "file",
            "javascript",
            "view-source",
            &"a".repeat(33),
        ] {
            assert!(scheme(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn allowed_hosts() {
        assert!(host("github.com").is_ok());
        assert!(host("xn--bcher-kva.de").is_ok());
        for bad in [
            "GitHub.com",
            "bücher.de",
            "github.com:443",
            "1.2.3.4",
            "[::1]",
            "a@b.com",
            "https://github.com",
            "github.com.",
            "a_b.com",
            "",
        ] {
            assert!(host(bad).is_err(), "{bad}");
        }
        assert_eq!(normalize_host(" Bücher.DE ").unwrap(), "xn--bcher-kva.de");
        assert!(normalize_host("10.0.0.1").is_err());
    }

    #[test]
    fn incoming_links() {
        let s = ["claude".to_owned()];
        assert_eq!(
            incoming("claude://login?code=1", &s).unwrap(),
            "claude://login?code=1"
        );
        assert!(incoming("CLAUDE://x", &s).is_ok());
        for bad in [
            "https://claude.ai",
            "claudex://x",
            "claude",
            "claude://a\nb",
            "claude://a\u{7f}",
            "--option=claude:",
        ] {
            assert!(incoming(bad, &s).is_err(), "{bad:?}");
        }
        assert!(incoming(&format!("claude://{}", "a".repeat(MAX_URL)), &s).is_err());
    }

    #[test]
    fn outgoing_schemes_and_serialisation() {
        let o = outgoing("https://Example.com/a b?q=1#f").unwrap();
        assert_eq!(o.url, "https://example.com/a%20b?q=1#f");
        assert_eq!(o.host, "example.com");
        assert!(o.marks.is_empty());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://x.org",
            "mailto:a@b.c",
            "claude://x",
            "https://a.com/\r\nHost: x",
            "https://a.com/\u{0}",
            "/relative",
            "",
        ] {
            assert!(outgoing(bad).is_err(), "{bad:?}");
        }
        assert!(outgoing(&format!("https://a.com/{}", "a".repeat(MAX_URL))).is_err());
    }

    #[test]
    fn suspicious_marks() {
        assert_eq!(
            marks("https://github.com@evil.com/x"),
            [Mark::UserInfo("evil.com".into())]
        );
        assert_eq!(
            marks("https://аpple.com"),
            [Mark::Idn("xn--pple-43d.com".into())]
        );
        assert_eq!(marks("http://93.184.216.34/"), [Mark::IpLiteral]);
        assert_eq!(marks("http://192.168.1.1/"), [Mark::IpLiteral, Mark::Local]);
        assert_eq!(
            marks("http://[::1]:8080/"),
            [Mark::IpLiteral, Mark::Local, Mark::Port(8080)]
        );
        assert_eq!(marks("http://[fe80::1]/"), [Mark::IpLiteral, Mark::Local]);
        assert_eq!(marks("http://localhost/"), [Mark::Local]);
        assert_eq!(marks("http://printer.local/"), [Mark::Local]);
        assert_eq!(marks("http://intranet/"), [Mark::Local]);
        // Numeric hosts in other notations are IPv4 for the parser, so for the marks too.
        assert_eq!(marks("http://2130706433/"), [Mark::IpLiteral, Mark::Local]);
        assert_eq!(marks("http://0x7f.1/"), [Mark::IpLiteral, Mark::Local]);
        assert_eq!(marks("https://a.com:8443/"), [Mark::Port(8443)]);
        assert!(marks("https://a.com:443/").is_empty());
        assert_eq!(
            marks(&format!("https://a.com/{}", "a".repeat(LONG_URL))),
            [Mark::Long]
        );
        assert_eq!(
            Mark::Port(8443).text(),
            "The link uses the unusual port 8443."
        );
    }
}
