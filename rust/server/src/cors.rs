//! CORS policy, matching the reference server.
//!
//! `GENERAL.CORS_ALLOWED_HOSTS` is read once at startup, exactly as
//! `server.py` does when it builds its `SocketIO` service:
//!
//! * empty (the default) - same-origin only. The reference implementation
//!   turns the empty string into `cors_allowed_origins=None`, which engineio
//!   resolves to `"<scheme>://<host>"` of the request, honouring
//!   `X-Forwarded-Proto` / `X-Forwarded-Host` when a proxy set them.
//! * `*` - any origin.
//! * anything else - the allowed origins, comma or whitespace separated.
//!   (Python treats a bare string as a single origin; splitting is a
//!   deliberate small improvement, and it can only widen what an operator
//!   already asked for.)
//!
//! engineio rejects a request carrying a foreign `Origin` outright with
//! `400 Not an accepted origin.`, and that is what this module's verdict
//! drives. Browsers do not apply CORS to WebSocket upgrades, so that check is
//! the only thing protecting the timer from a foreign page opening a socket
//! and driving the race.

/// What the config said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsPolicy {
    /// Empty config: only the origin the request was served from.
    SameOrigin,
    /// `*`: no restriction.
    Any,
    /// An explicit allow list.
    Origins(Vec<String>),
}

impl CorsPolicy {
    /// Parse `GENERAL.CORS_ALLOWED_HOSTS`.
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if raw.is_empty() {
            return CorsPolicy::SameOrigin;
        }
        if raw == "*" {
            return CorsPolicy::Any;
        }
        let origins: Vec<String> = raw
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .collect();
        if origins.is_empty() {
            CorsPolicy::SameOrigin
        } else if origins.iter().any(|o| o == "*") {
            CorsPolicy::Any
        } else {
            CorsPolicy::Origins(origins)
        }
    }

    /// Decide one request.
    ///
    /// `scheme`/`host` describe how the client reached us; the forwarded
    /// values are the first entry of `X-Forwarded-Proto` / `X-Forwarded-Host`
    /// when present, because a reverse proxy is the normal deployment.
    pub fn check(
        &self,
        origin: Option<&str>,
        scheme: &str,
        host: Option<&str>,
        forwarded_proto: Option<&str>,
        forwarded_host: Option<&str>,
    ) -> Verdict {
        let origin = origin.map(str::trim).filter(|o| !o.is_empty());

        // With a wildcard there is nothing to compare, so a caller that sent
        // an Origin gets it echoed (engineio does the same: a literal "*" is
        // illegal next to Access-Control-Allow-Credentials) and a caller
        // without one is answered "*" - it is not a CORS request at all.
        if *self == CorsPolicy::Any {
            return Verdict::Allowed(origin.map(str::to_string).unwrap_or_else(|| "*".to_string()));
        }

        let Some(origin) = origin else {
            // A navigation, curl, or a Python client. Nothing to check -
            // CORS is a browser concept.
            return Verdict::NoOrigin;
        };
        // "null" is what a sandboxed iframe or a file:// page sends. It is
        // never a legitimate timer client.
        if origin == "null" {
            return Verdict::Denied;
        }

        match self {
            CorsPolicy::Any => unreachable!("handled above"),
            CorsPolicy::Origins(list) => {
                let trimmed = origin.trim_end_matches('/');
                if list.iter().any(|o| o == trimmed) {
                    Verdict::Allowed(origin.to_string())
                } else {
                    Verdict::Denied
                }
            }
            CorsPolicy::SameOrigin => {
                let forwarded = match (forwarded_proto, forwarded_host) {
                    (Some(p), Some(h)) => Some((first_value(p), first_value(h))),
                    (Some(p), None) => host.map(|h| (first_value(p), h)),
                    (None, Some(h)) => Some((scheme, first_value(h))),
                    (None, None) => None,
                };
                let mut allowed: Vec<String> = Vec::new();
                if let Some(h) = host {
                    for s in ["http", "https"] {
                        allowed.push(format!("{}://{}", s, h));
                    }
                }
                if let Some((p, h)) = forwarded {
                    allowed.push(format!("{}://{}", p, h));
                }
                let trimmed = origin.trim_end_matches('/');
                if allowed.iter().any(|a| a.trim_end_matches('/') == trimmed) {
                    Verdict::Allowed(origin.to_string())
                } else {
                    Verdict::Denied
                }
            }
        }
    }
}

fn first_value(raw: &str) -> &str {
    raw.split(',').next().unwrap_or(raw).trim()
}

/// The outcome for a single request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// No Origin header at all and nothing to advertise.
    NoOrigin,
    /// Allowed; the value is what belongs in `Access-Control-Allow-Origin`
    /// (`*` only when the caller sent no Origin at all).
    Allowed(String),
    /// A browser origin we do not trust.
    Denied,
}

impl Verdict {
    pub fn is_denied(&self) -> bool {
        matches!(self, Verdict::Denied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_config_means_same_origin_only() {
        assert_eq!(CorsPolicy::parse(""), CorsPolicy::SameOrigin);
        assert_eq!(CorsPolicy::parse("   "), CorsPolicy::SameOrigin);
        // A list of nothing but separators is still "same origin", never "any".
        assert_eq!(CorsPolicy::parse(" , "), CorsPolicy::SameOrigin);
    }

    #[test]
    fn a_star_allows_everything() {
        assert_eq!(CorsPolicy::parse("*"), CorsPolicy::Any);
        assert_eq!(CorsPolicy::parse(" * "), CorsPolicy::Any);
        assert_eq!(
            CorsPolicy::parse("https://a.example,*"),
            CorsPolicy::Any,
            "a list containing * is the same as *"
        );
    }

    #[test]
    fn an_allow_list_is_split_and_normalised() {
        let policy = CorsPolicy::parse("https://a.example, https://b.example/");
        assert_eq!(
            policy,
            CorsPolicy::Origins(vec![
                "https://a.example".to_string(),
                "https://b.example".to_string()
            ])
        );
        assert_eq!(
            CorsPolicy::parse("https://a.example https://b.example"),
            CorsPolicy::Origins(vec![
                "https://a.example".to_string(),
                "https://b.example".to_string()
            ]),
            "whitespace separated lists are the same list"
        );
    }

    #[test]
    fn same_origin_compares_scheme_and_host() {
        let policy = CorsPolicy::SameOrigin;
        assert_eq!(
            policy.check(Some("http://127.0.0.1:5000"), "http", Some("127.0.0.1:5000"), None, None),
            Verdict::Allowed("http://127.0.0.1:5000".into())
        );
        assert_eq!(
            policy.check(Some("https://timer.local"), "https", Some("timer.local"), None, None),
            Verdict::Allowed("https://timer.local".into())
        );
        assert_eq!(
            policy.check(Some("http://evil.example"), "http", Some("127.0.0.1:5000"), None, None),
            Verdict::Denied
        );
        // Port mismatch is a different origin, not a variant of the same one.
        assert_eq!(
            policy.check(Some("http://127.0.0.1:5001"), "http", Some("127.0.0.1:5000"), None, None),
            Verdict::Denied
        );
    }

    #[test]
    fn forwarded_headers_extend_same_origin() {
        let policy = CorsPolicy::SameOrigin;
        assert_eq!(
            policy.check(
                Some("https://timer.example.com"),
                "http",
                Some("127.0.0.1:5000"),
                Some("https, http"),
                Some("timer.example.com, proxy.internal")
            ),
            Verdict::Allowed("https://timer.example.com".into())
        );
    }

    #[test]
    fn requests_without_an_origin_are_not_cors_requests() {
        let policy = CorsPolicy::SameOrigin;
        assert_eq!(
            policy.check(None, "http", Some("127.0.0.1:5000"), None, None),
            Verdict::NoOrigin
        );
        // "null" (sandboxed iframe, file://) is never the timer itself.
        assert_eq!(
            policy.check(Some("null"), "http", Some("127.0.0.1:5000"), None, None),
            Verdict::Denied
        );
    }

    #[test]
    fn an_allow_list_accepts_only_its_entries() {
        let policy = CorsPolicy::parse("https://a.example");
        assert_eq!(
            policy.check(Some("https://a.example"), "http", Some("127.0.0.1:5000"), None, None),
            Verdict::Allowed("https://a.example".into())
        );
        assert_eq!(
            policy.check(Some("https://b.example"), "http", Some("127.0.0.1:5000"), None, None),
            Verdict::Denied
        );
    }

    #[test]
    fn a_wildcard_echoes_the_caller_and_stars_a_bare_request() {
        let policy = CorsPolicy::parse("*");
        assert_eq!(
            policy.check(Some("https://whatever.example"), "http", Some("127.0.0.1:5000"), None, None),
            Verdict::Allowed("https://whatever.example".into()),
            "a literal * cannot travel with credentials"
        );
        assert_eq!(
            policy.check(None, "http", Some("127.0.0.1:5000"), None, None),
            Verdict::Allowed("*".into())
        );
    }
}
