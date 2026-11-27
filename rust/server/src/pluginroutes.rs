//! Plugin HTTP routes: the rules a plugin's blueprint registered, and matching
//! a request path against them.
//!
//! Python mounts a plugin's blueprint on the Flask app at the root, so rules are
//! absolute (`/stream/overlay/<name>/node/<int:node_id>`). The bridge registers
//! each rule here when the plugin calls `blueprint_add`; a request that matches is
//! forwarded to the plugin's own Python, so the view function runs exactly as the
//! plugin wrote it.

use std::collections::HashMap;

use serde_json::Value;

/// One registered rule: which plugin owns it, the pattern, and its methods.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub domain: String,
    pub rule: String,
    pub methods: Vec<String>,
}

impl Route {
    pub fn from_json(value: &Value) -> Option<Route> {
        Some(Route {
            domain: value.get("domain")?.as_str().unwrap_or("").to_string(),
            rule: value.get("rule")?.as_str()?.to_string(),
            methods: value
                .get("methods")
                .and_then(|methods| methods.as_array())
                .map(|methods| {
                    methods
                        .iter()
                        .filter_map(|method| method.as_str().map(|m| m.to_uppercase()))
                        .collect()
                })
                .unwrap_or_else(|| vec!["GET".to_string()]),
        })
    }

    pub fn allows(&self, method: &str) -> bool {
        let method = method.to_uppercase();
        self.methods.iter().any(|allowed| *allowed == method)
            || (method == "HEAD" && self.methods.iter().any(|allowed| allowed == "GET"))
    }

    /// `(params)` when the rule matches the path, `None` otherwise.
    pub fn match_path(&self, path: &str) -> Option<HashMap<String, String>> {
        match_rule(&self.rule, path)
    }
}

fn segments(value: &str) -> Vec<&str> {
    value
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect()
}

/// Flask-style matching: `<int:name>`, `<float:name>`, `<string:name>`,
/// `<path:name>` (greedy, may contain slashes) and bare `<name>` (string).
pub fn match_rule(rule: &str, path: &str) -> Option<HashMap<String, String>> {
    let rule_segments = segments(rule);
    let path_segments = segments(path);
    let mut params = HashMap::new();
    let mut index = 0usize;

    for (position, segment) in rule_segments.iter().enumerate() {
        if let Some(spec) = segment
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
        {
            let (converter, name) = match spec.split_once(':') {
                Some((converter, name)) => (converter, name),
                None => ("string", spec),
            };
            if converter == "path" {
                // greedy: everything from here to the end
                let rest = path_segments.get(position..).unwrap_or(&[]).join("/");
                if rest.is_empty() {
                    return None;
                }
                params.insert(name.to_string(), rest);
                index = path_segments.len();
                return if index == path_segments.len() {
                    Some(params)
                } else {
                    None
                };
            }
            let value = path_segments.get(position)?;
            match converter {
                "int" => {
                    value.parse::<i64>().ok()?;
                }
                "float" => {
                    value.parse::<f64>().ok()?;
                }
                _ => {
                    if value.is_empty() {
                        return None;
                    }
                }
            }
            params.insert(name.to_string(), value.to_string());
        } else if path_segments.get(position) != Some(segment) {
            return None;
        }
        index = position + 1;
    }

    if index == path_segments.len() {
        Some(params)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_static_rule_matches_exactly_one_path() {
        assert!(match_rule("/fixture/ping", "/fixture/ping").is_some());
        assert!(match_rule("/fixture/ping", "/fixture/ping/extra").is_none());
        assert!(match_rule("/fixture/ping", "/fixture").is_none());
        assert!(match_rule("/fixture/ping", "/other/ping").is_none());
        assert!(match_rule("/", "/").is_some());
    }

    #[test]
    fn typed_converters_extract_and_validate() {
        let params = match_rule("/a/<int:number>/b", "/a/42/b").expect("int matches");
        assert_eq!(params.get("number").map(String::as_str), Some("42"));
        assert!(match_rule("/a/<int:number>", "/a/forty-two").is_none());

        let params = match_rule("/f/<float:ratio>", "/f/1.5").expect("float matches");
        assert_eq!(params.get("ratio").map(String::as_str), Some("1.5"));

        let params = match_rule("/s/<name>", "/s/abc").expect("bare name is a string");
        assert_eq!(params.get("name").map(String::as_str), Some("abc"));
    }

    #[test]
    fn a_path_converter_swallows_slashes() {
        let params = match_rule("/files/<path:rest>", "/files/a/b/c.txt").expect("matches");
        assert_eq!(params.get("rest").map(String::as_str), Some("a/b/c.txt"));
        assert!(match_rule("/files/<path:rest>", "/files").is_none());
    }

    #[test]
    fn a_real_plugin_rule_matches_its_real_url() {
        // straight from rh-stream-overlays
        let params = match_rule(
            "/stream/overlay/<string:name>/node/<int:node_id>",
            "/stream/overlay/main/node/3",
        )
        .expect("matches");
        assert_eq!(params.get("name").map(String::as_str), Some("main"));
        assert_eq!(params.get("node_id").map(String::as_str), Some("3"));
        assert!(match_rule(
            "/stream/overlay/<string:name>/node/<int:node_id>",
            "/stream/overlay/main/node/three"
        )
        .is_none());
    }

    #[test]
    fn methods_are_respected_and_head_follows_get() {
        let route = Route {
            domain: "fixture".into(),
            rule: "/x".into(),
            methods: vec!["GET".into()],
        };
        assert!(route.allows("get"));
        assert!(route.allows("HEAD"));
        assert!(!route.allows("POST"));
    }
}
