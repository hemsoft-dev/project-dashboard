//! The browser boundary for URLs supplied by hosted data, local T3 state, and old caches.

/// A parsed HTTP(S) URL. Construction is the only way data can become an openable link.
#[derive(Clone, Debug)]
pub struct WebUrl(reqwest::Url);

impl WebUrl {
    /// Require an explicit web authority; do not repair paths, whitespace or backslashes into URLs.
    pub fn parse(raw: &str) -> Option<Self> {
        let (scheme, rest) = raw.split_once("://")?;
        if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            || raw.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\')
            || rest.split(['/', '?', '#']).next()?.is_empty()
        {
            return None;
        }
        let url = reqwest::Url::parse(raw).ok()?;
        (matches!(url.scheme(), "http" | "https") && url.has_host()).then_some(Self(url))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::WebUrl;

    #[test]
    fn accepts_web_and_local_t3_links_with_mixed_case_schemes() {
        for raw in [
            "https://github.com/hemsoft-dev/project-dashboard/issues/67",
            "HtTpS://example.com/a?q=b#c",
            "http://127.0.0.1:3773/thread/abc",
            "HTTP://localhost:3773/",
            "http://[::1]:3773/",
        ] {
            assert!(WebUrl::parse(raw).is_some(), "{raw}");
        }
    }

    #[test]
    fn refuses_non_web_and_malformed_web_values() {
        for raw in [
            r"C:\apps\x.exe",
            r"\\host\share\x.exe",
            "file:///C:/x.exe",
            "javascript:alert(1)",
            "ms-settings:display",
            "",
            "example.com",
            "//example.com",
            "https:example.com",
            "https:/example.com",
            "https:///example.com",
            "https://",
            "http://?q=x",
            "http://#x",
            "https://[broken",
            "http://host:bad/",
            "http://host:99999/",
            " https://example.com",
            "https://exa mple.com",
            "https://example.com/\n",
            r"https://example.com\x",
        ] {
            assert!(WebUrl::parse(raw).is_none(), "{raw:?}");
        }
    }
}
