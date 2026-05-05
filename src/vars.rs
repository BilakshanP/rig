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
    /// Set when running a `.rig` bundle: absolute path to the staging root.
    /// Resolved as `{{#bundle}}`. `None` outside bundle runs, in which case
    /// `{{#bundle}}` stays as a literal template (same behavior as any other
    /// unresolved reference).
    bundle_root: Option<String>,
}

impl Scope {
    pub fn new(startup_ts: chrono::DateTime<chrono::Local>, startup_pwd: String) -> Self {
        Self {
            values: HashMap::new(),
            startup_ts: Some(startup_ts),
            startup_pwd: Some(startup_pwd),
            bundle_root: None,
        }
    }

    /// Record the bundle staging root so `{{#bundle}}` resolves to it.
    ///
    /// Called by the CLI immediately after `open_bundle` succeeds; leaves the
    /// field `None` for plain-config runs so `{{#bundle}}` stays unresolved
    /// (and therefore prints literally) in that case.
    pub fn set_bundle_root(&mut self, path: String) {
        self.bundle_root = Some(path);
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
            "bundle" => self.bundle_root.clone(),
            "os" => Some(std::env::consts::OS.to_string()),
            "arch" => Some(std::env::consts::ARCH.to_string()),
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
        // Scan left-to-right emitting plain text until we hit `{{`.
        // Escapes: `{{{{` → literal `{{`, `}}}}` → literal `}}`. Keeps the
        // template syntax usable inside strings that themselves need to
        // contain literal double-brace pairs (e.g., a path with a literal
        // `{{name}}` directory segment).
        //
        // Byte-indexed scan is safe because `{` and `}` are ASCII and any
        // non-ASCII UTF-8 byte is never equal to 0x7b/0x7d. We only ever
        // slice at positions we've confirmed are ASCII boundaries (4-byte
        // escape markers and the two-byte `{{` opener) and at the index
        // returned by `find` which is a char-boundary.
        let mut out = String::with_capacity(s.len());
        let bytes = s.as_bytes();
        let mut i = 0usize;
        let mut plain_start = 0usize;

        let flush_plain = |out: &mut String, plain_start: usize, upto: usize| {
            if plain_start < upto {
                out.push_str(&s[plain_start..upto]);
            }
        };

        while i < bytes.len() {
            if bytes[i..].starts_with(b"{{{{") {
                flush_plain(&mut out, plain_start, i);
                out.push_str("{{");
                i += 4;
                plain_start = i;
                continue;
            }
            if bytes[i..].starts_with(b"}}}}") {
                flush_plain(&mut out, plain_start, i);
                out.push_str("}}");
                i += 4;
                plain_start = i;
                continue;
            }
            if bytes[i..].starts_with(b"{{") {
                flush_plain(&mut out, plain_start, i);
                let body_start = i + 2;
                let rest_bytes = &bytes[body_start..];
                if let Some(end_rel) = find_subsequence(rest_bytes, b"}}") {
                    let end = body_start + end_rel;
                    let expr = &s[body_start..end];
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
                    i = end + 2;
                    plain_start = i;
                    continue;
                } else {
                    // No closing `}}` — pass through the remainder verbatim.
                    out.push_str(&s[i..]);
                    return out;
                }
            }
            i += 1;
        }
        flush_plain(&mut out, plain_start, bytes.len());
        out
    }
}

/// Locate `needle` within `haystack` by byte comparison. Small helper so the
/// scanner doesn't have to re-slice UTF-8 boundaries; the inputs we search
/// for (`{{` and `}}`) are ASCII so byte search is safe.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
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

/// Scan a string for all `{{...}}` variable references. Escaped `{{{{` (and
/// `}}}}`) sequences are treated as literals and do not contribute a ref.
pub fn scan_refs(s: &str) -> Vec<VarRef> {
    let mut refs = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"{{{{") {
            // Literal `{{` — skip the escape.
            i += 4;
            continue;
        }
        if bytes[i..].starts_with(b"}}}}") {
            i += 4;
            continue;
        }
        if bytes[i..].starts_with(b"{{") {
            let body_start = i + 2;
            let rest_bytes = &bytes[body_start..];
            if let Some(end_rel) = find_subsequence(rest_bytes, b"}}") {
                let end = body_start + end_rel;
                if let Some(vr) = VarRef::parse(&s[body_start..end]) {
                    refs.push(vr);
                }
                i = end + 2;
                continue;
            } else {
                break;
            }
        }
        i += 1;
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

    #[test]
    fn scope_resolves_bundle_builtin_when_set() {
        let mut s = Scope::new(chrono::Local::now(), "/tmp".into());
        s.set_bundle_root("/tmp/stage-xyz".into());
        assert_eq!(s.substitute("path: {{#bundle}}/x"), "path: /tmp/stage-xyz/x");
    }

    #[test]
    fn scope_bundle_builtin_unresolved_without_bundle() {
        // Outside a bundle run, `{{#bundle}}` stays literal (same behavior
        // as any other unresolved reference) so misuse is visible.
        let s = Scope::new(chrono::Local::now(), "/tmp".into());
        assert_eq!(s.substitute("path: {{#bundle}}/x"), "path: {{#bundle}}/x");
    }

    #[test]
    fn escape_produces_literal_double_braces() {
        let mut s = Scope::new(chrono::Local::now(), "/tmp".into());
        s.set("name", "world".into());
        // `{{{{name}}}}` → literal `{{name}}`.
        assert_eq!(s.substitute("hi {{{{name}}}}"), "hi {{name}}");
    }

    #[test]
    fn escape_coexists_with_real_substitution() {
        let mut s = Scope::new(chrono::Local::now(), "/tmp".into());
        s.set_bundle_root("/stage".into());
        // `{{#bundle}}` resolves, neighboring `{{{{name}}}}` stays literal.
        assert_eq!(
            s.substitute("{{#bundle}}/{{{{name}}}}/pyproject.toml"),
            "/stage/{{name}}/pyproject.toml"
        );
    }

    #[test]
    fn scan_refs_skips_escaped_sequences() {
        let refs = scan_refs("{{real}} and {{{{literal}}}} done");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].key(), "real");
    }

    #[test]
    fn escape_only_when_exactly_four_braces() {
        let mut s = Scope::new(chrono::Local::now(), "/tmp".into());
        s.set("x", "X".into());
        // Three braces on the left: the scanner grabs the first pair as a
        // template opener, leaving `{x` as the expression. That isn't a
        // valid var name (leading `{`) so the whole `{{...}}` stays literal.
        assert_eq!(s.substitute("{{{x}}"), "{{{x}}");
    }
}
