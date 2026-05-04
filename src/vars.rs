//! Variable system: parsing, classification, scope management, substitution.
//!
//! # Variable categories (by prefix + first-char case of first path segment)
//!
//! - `#NAME`        - Built-in (timestamp/now/pwd); read-only
//! - `@NAME`        - Runtime-only mutable (not CLI-settable)
//! - `@name`        - Mutable via var action OR --set
//! - `NAME`         - Immutable constant from meta.vars
//! - `name`         - Set via meta.vars or --set; immutable at runtime
//!
//! # Syntax
//! `{{[#|@]name[.nested.path][:format]}}`
//!
//! Format specifier (`:format`) is only meaningful for `#timestamp` and `#now`.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Prefix {
    None,
    Hash,
    At,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VarKind {
    /// `#timestamp`, `#pwd` - resolved at startup, stored in scope.
    BuiltinStartup,
    /// `#now` - resolved at each use (ignores scope cache).
    BuiltinRuntime,
    /// `NAME` - immutable constant from meta.vars.
    ConstUpper,
    /// `name` - CLI-settable constant (via --set), immutable at runtime.
    ConstLower,
    /// `@NAME` - runtime-mutable, not CLI-settable.
    MutableUpper,
    /// `@name` - CLI-settable and runtime-mutable.
    MutableLower,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarRef {
    pub prefix: Prefix,
    /// Dot-separated path segments: `{{super.mario.bros}}` -> ["super", "mario", "bros"].
    pub path: Vec<String>,
    /// Optional format specifier after ':'. Only for `#timestamp`/`#now`.
    pub format: Option<String>,
}

impl VarRef {
    /// Parse a variable reference expression (without the surrounding `{{` `}}`).
    /// Returns None if invalid.
    pub fn parse(s: &str) -> Option<Self> {
        let (prefix, rest) = if let Some(r) = s.strip_prefix('#') {
            (Prefix::Hash, r)
        } else if let Some(r) = s.strip_prefix('@') {
            (Prefix::At, r)
        } else {
            (Prefix::None, s)
        };

        let (name_part, format) = match rest.find(':') {
            Some(idx) => (&rest[..idx], Some(rest[idx + 1..].to_string())),
            None => (rest, None),
        };

        if name_part.is_empty() { return None; }

        let mut path = Vec::new();
        for seg in name_part.split('.') {
            if !is_valid_ident(seg) { return None; }
            path.push(seg.to_string());
        }

        Some(VarRef { prefix, path, format })
    }

    /// Classify this var reference based on prefix and first-segment case.
    pub fn kind(&self) -> VarKind {
        let first = self.path.first().map(|s| s.chars().next().unwrap_or('_')).unwrap_or('_');
        let upper = first.is_ascii_uppercase();
        match (&self.prefix, self.path.first().map(|s| s.as_str())) {
            (Prefix::Hash, Some("now")) => VarKind::BuiltinRuntime,
            (Prefix::Hash, _) => VarKind::BuiltinStartup,
            (Prefix::At, _) if upper => VarKind::MutableUpper,
            (Prefix::At, _) => VarKind::MutableLower,
            (Prefix::None, _) if upper => VarKind::ConstUpper,
            (Prefix::None, _) => VarKind::ConstLower,
        }
    }

    /// Joined dot-path for lookup: `["super", "mario", "bros"]` -> `"super.mario.bros"`.
    pub fn key(&self) -> String {
        self.path.join(".")
    }

    /// Full canonical form including prefix (for error messages).
    pub fn display(&self) -> String {
        let p = match self.prefix {
            Prefix::None => "",
            Prefix::Hash => "#",
            Prefix::At => "@",
        };
        let fmt = self.format.as_deref().map(|f| format!(":{f}")).unwrap_or_default();
        format!("{p}{}{fmt}", self.key())
    }

    /// Whether this var is writable at runtime (via `var` action).
    pub fn is_runtime_writable(&self) -> bool {
        matches!(self.kind(), VarKind::MutableUpper | VarKind::MutableLower)
    }
}

fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false; };
    if !(first.is_ascii_alphabetic() || first == '_') { return false; }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Runtime variable scope.
#[derive(Debug, Default, Clone)]
pub struct Scope {
    values: HashMap<String, String>,
    /// Precomputed `#timestamp` (and `#pwd`) for this run.
    startup_ts: Option<chrono::DateTime<chrono::Local>>,
    startup_pwd: Option<String>,
}

impl Scope {
    pub fn new(startup_ts: chrono::DateTime<chrono::Local>, startup_pwd: String) -> Self {
        Self {
            values: HashMap::new(),
            startup_ts: Some(startup_ts),
            startup_pwd: Some(startup_pwd),
        }
    }

    /// Insert a value under the given dot-path key.
    pub fn set(&mut self, key: &str, value: String) {
        self.values.insert(key.to_string(), value);
    }

    /// Look up a value by dot-path key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }

    /// Resolve a parsed var reference to its string value.
    /// Returns None if undefined.
    pub fn resolve(&self, vr: &VarRef) -> Option<String> {
        match vr.kind() {
            VarKind::BuiltinStartup => self.resolve_builtin_startup(vr),
            VarKind::BuiltinRuntime => self.resolve_builtin_runtime(vr),
            _ => self.get(&vr.key()).map(|s| s.to_string()),
        }
    }

    fn resolve_builtin_startup(&self, vr: &VarRef) -> Option<String> {
        match vr.path.first().map(|s| s.as_str())? {
            "timestamp" => {
                let ts = self.startup_ts?;
                let fmt = vr.format.as_deref().unwrap_or("%Y%m%dT%H%M%S");
                Some(ts.format(fmt).to_string())
            }
            "pwd" => self.startup_pwd.clone(),
            _ => None,
        }
    }

    fn resolve_builtin_runtime(&self, vr: &VarRef) -> Option<String> {
        match vr.path.first().map(|s| s.as_str())? {
            "now" => {
                let fmt = vr.format.as_deref().unwrap_or("%Y%m%dT%H%M%S");
                Some(chrono::Local::now().format(fmt).to_string())
            }
            _ => None,
        }
    }

    /// Substitute all `{{...}}` references in a string using this scope.
    /// Unresolved references are left as-is (caller may treat as error).
    pub fn substitute(&self, s: &str) -> String {
        self.substitute_impl(s, false)
    }

    /// Substitute like `substitute`, but wrap any unresolved `{{...}}` in yellow
    /// aml markup for terminal display. Use this only for strings that will be
    /// passed to `style::render` (e.g., io messages).
    pub fn substitute_display(&self, s: &str) -> String {
        self.substitute_impl(s, true)
    }

    fn substitute_impl(&self, s: &str, highlight_unresolved: bool) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            if let Some(end) = after.find("}}") {
                let expr = &after[..end];
                if let Some(vr) = VarRef::parse(expr)
                    && let Some(val) = self.resolve(&vr)
                {
                    out.push_str(&val);
                } else if highlight_unresolved {
                    out.push_str("<fy>{{");
                    out.push_str(expr);
                    out.push_str("}}</f>");
                } else {
                    out.push_str("{{");
                    out.push_str(expr);
                    out.push_str("}}");
                }
                rest = &after[end + 2..];
            } else {
                out.push_str("{{");
                out.push_str(after);
                break;
            }
        }
        out.push_str(rest);
        out
    }
}

/// Flatten a nested JSON object into dot-path keys with string values.
/// Non-string leaves are stringified.
pub fn flatten_vars(value: &serde_json::Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let serde_json::Value::Object(obj) = value {
        for (k, v) in obj {
            flatten_inner(k, v, &mut out);
        }
    }
    out
}

fn flatten_inner(prefix: &str, value: &serde_json::Value, out: &mut HashMap<String, String>) {
    match value {
        serde_json::Value::Object(obj) => {
            for (k, v) in obj {
                let new_prefix = format!("{prefix}.{k}");
                flatten_inner(&new_prefix, v, out);
            }
        }
        serde_json::Value::String(s) => { out.insert(prefix.to_string(), s.clone()); }
        serde_json::Value::Number(n) => { out.insert(prefix.to_string(), n.to_string()); }
        serde_json::Value::Bool(b) => { out.insert(prefix.to_string(), b.to_string()); }
        _ => {}
    }
}

/// Scan a string for all `{{...}}` variable references.
pub fn scan_refs(s: &str) -> Vec<VarRef> {
    let mut refs = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            if let Some(vr) = VarRef::parse(&after[..end]) {
                refs.push(vr);
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let vr = VarRef::parse("name").unwrap();
        assert_eq!(vr.prefix, Prefix::None);
        assert_eq!(vr.path, vec!["name"]);
    }

    #[test]
    fn parse_with_prefix() {
        let vr = VarRef::parse("@var").unwrap();
        assert_eq!(vr.prefix, Prefix::At);
        assert_eq!(vr.path, vec!["var"]);

        let vr = VarRef::parse("#timestamp").unwrap();
        assert_eq!(vr.prefix, Prefix::Hash);
    }

    #[test]
    fn parse_nested() {
        let vr = VarRef::parse("super.mario.bros").unwrap();
        assert_eq!(vr.path, vec!["super", "mario", "bros"]);
        assert_eq!(vr.key(), "super.mario.bros");
    }

    #[test]
    fn parse_with_format() {
        let vr = VarRef::parse("#timestamp:%Y-%m-%d").unwrap();
        assert_eq!(vr.prefix, Prefix::Hash);
        assert_eq!(vr.path, vec!["timestamp"]);
        assert_eq!(vr.format.as_deref(), Some("%Y-%m-%d"));
    }

    #[test]
    fn parse_rejects_invalid() {
        assert!(VarRef::parse("").is_none());
        assert!(VarRef::parse("1name").is_none());
        assert!(VarRef::parse("name with space").is_none());
        assert!(VarRef::parse("a..b").is_none());
    }

    #[test]
    fn kind_classification() {
        assert_eq!(VarRef::parse("#timestamp").unwrap().kind(), VarKind::BuiltinStartup);
        assert_eq!(VarRef::parse("#now").unwrap().kind(), VarKind::BuiltinRuntime);
        assert_eq!(VarRef::parse("#pwd").unwrap().kind(), VarKind::BuiltinStartup);
        assert_eq!(VarRef::parse("@NAME").unwrap().kind(), VarKind::MutableUpper);
        assert_eq!(VarRef::parse("@name").unwrap().kind(), VarKind::MutableLower);
        assert_eq!(VarRef::parse("NAME").unwrap().kind(), VarKind::ConstUpper);
        assert_eq!(VarRef::parse("name").unwrap().kind(), VarKind::ConstLower);
        // Nested: category from first segment
        assert_eq!(VarRef::parse("@Super.mario").unwrap().kind(), VarKind::MutableUpper);
        assert_eq!(VarRef::parse("super.Mario").unwrap().kind(), VarKind::ConstLower);
    }

    #[test]
    fn flatten_nested_vars() {
        let v: serde_json::Value = serde_json::from_str(r#"{
            "super": { "mario": { "bros": "smb", "sis": "sms" } },
            "flat": "f"
        }"#).unwrap();
        let f = flatten_vars(&v);
        assert_eq!(f.get("super.mario.bros").unwrap(), "smb");
        assert_eq!(f.get("super.mario.sis").unwrap(), "sms");
        assert_eq!(f.get("flat").unwrap(), "f");
    }

    #[test]
    fn scope_resolves_plain_var() {
        let mut s = Scope::new(chrono::Local::now(), "/tmp".into());
        s.set("greeting", "hello".into());
        let r = VarRef::parse("greeting").unwrap();
        assert_eq!(s.resolve(&r).as_deref(), Some("hello"));
    }

    #[test]
    fn scope_resolves_nested() {
        let mut s = Scope::new(chrono::Local::now(), "/tmp".into());
        s.set("super.mario.bros", "smb".into());
        let r = VarRef::parse("super.mario.bros").unwrap();
        assert_eq!(s.resolve(&r).as_deref(), Some("smb"));
    }

    #[test]
    fn scope_substitute_string() {
        let mut s = Scope::new(chrono::Local::now(), "/tmp".into());
        s.set("name", "world".into());
        assert_eq!(s.substitute("hello {{name}}!"), "hello world!");
    }

    #[test]
    fn scope_substitute_builtin_pwd() {
        let s = Scope::new(chrono::Local::now(), "/tmp/test".into());
        assert_eq!(s.substitute("in {{#pwd}}"), "in /tmp/test");
    }

    #[test]
    fn scope_substitute_leaves_unresolved() {
        let s = Scope::new(chrono::Local::now(), "/tmp".into());
        assert_eq!(s.substitute("hi {{@missing}}"), "hi {{@missing}}");
    }

    #[test]
    fn scan_refs_finds_all() {
        let refs = scan_refs("{{a}} and {{@b}} with {{#now:%H}}");
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].key(), "a");
        assert_eq!(refs[1].prefix, Prefix::At);
        assert_eq!(refs[2].format.as_deref(), Some("%H"));
    }

    #[test]
    fn runtime_writable_only_at_prefix() {
        assert!(VarRef::parse("@name").unwrap().is_runtime_writable());
        assert!(VarRef::parse("@NAME").unwrap().is_runtime_writable());
        assert!(!VarRef::parse("name").unwrap().is_runtime_writable());
        assert!(!VarRef::parse("NAME").unwrap().is_runtime_writable());
        assert!(!VarRef::parse("#timestamp").unwrap().is_runtime_writable());
    }
}
