//! `.rig` bundle format: tar.gz archive with a `manifest.json[c]` at the root
//! plus arbitrary template files that can be referenced via `fs.copy`.
//!
//! A bundle pairs a rig config (the manifest) with the files it needs, so large
//! configs don't have to inline every file's contents as escaped JSON strings.
//!
//! # Layout
//!
//! ```text
//! my-setup.rig                        # tar.gz archive, renamed .rig
//! ├── manifest.jsonc                  # the rig config, with optional `bundle` section
//! ├── pyproject.toml                  # referenced via `fs.copy from: "pyproject.toml"`
//! ├── src/
//! │   └── lib.rs
//! └── {{name}}/                       # literal dir name; path-expansion is opt-in via
//!     └── ...                         # `fs.copy` + `expand: {from: true, to: true}`
//! ```
//!
//! # Runtime model
//!
//! `rig foo.rig` extracts the archive into a staging directory (chosen by
//! `bundle.extract-to`), loads `manifest.jsonc` as the rig config, and runs it
//! with a [`BundleCtx`] attached to the executor. Inside the executor, `fs.copy`
//! reads its source relative to the staging root, renders `{{...}}` in file
//! contents (unless the path matches a `bundle.binary` glob), and writes the
//! result to the destination.
//!
//! This module owns:
//! - archive read/write (`pack`, `unpack`, `info`),
//! - bundle discovery (`open_bundle`),
//! - staging-dir lifecycle (`BundleCtx` + cleanup policy).

use serde::Deserialize;
use std::path::{Path, PathBuf};

// -- Public types --

/// Bundle-specific metadata declared in the manifest under the `bundle` key.
///
/// All fields are optional; defaults give a sensible "stage to tmp, clean up on
/// success, nothing is binary" behavior.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMeta {
    #[serde(default, rename = "extract-to")]
    pub extract_to: ExtractTo,
    #[serde(default)]
    pub cleanup: Cleanup,
    /// Glob patterns (relative to bundle root) for files that should be copied
    /// byte-for-byte without variable substitution. Everything else is treated
    /// as templated text.
    #[serde(default)]
    pub binary: Vec<String>,
}

/// Where the bundle should be extracted before running.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase", untagged)]
pub enum ExtractTo {
    /// One of the named locations: `"tmp"` (default), `"cwd"`, `"home"`.
    Named(#[serde(default)] NamedExtractTo),
    /// `{ "path": "/some/where" }` — extract to an explicit path.
    Custom {
        path: String,
    },
}

impl Default for ExtractTo {
    fn default() -> Self {
        Self::Named(NamedExtractTo::Tmp)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NamedExtractTo {
    #[default]
    Tmp,
    Cwd,
    Home,
}

/// When to remove the staging directory after the bundle run.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Cleanup {
    /// Delete staging unconditionally after the run.
    Always,
    /// Delete only when the run succeeded; keep on failure so the user can inspect.
    #[default]
    OnSuccess,
    /// Never delete; print the staging path after the run.
    Never,
}

/// Runtime bundle context: the staged filesystem plus the rules the executor
/// needs to honor (binary globs, cleanup policy).
///
/// Stored on the `Runner` when running from a bundle; absent otherwise.
///
/// Dropping a `BundleCtx` triggers its cleanup policy:
/// - `Always`    → the staging dir is removed unconditionally.
/// - `OnSuccess` → removed iff `mark_success()` was called before drop.
/// - `Never`     → left in place; the path is printed by the CLI caller.
#[derive(Debug)]
pub struct BundleCtx {
    /// Absolute path to the staging directory. Relative `fs.copy.from` paths
    /// are resolved against this root when running a bundle.
    pub root: PathBuf,
    /// Compiled glob matcher over the bundle's `binary` patterns.
    pub binary: BinaryMatcher,
    /// Cleanup policy to apply on exit.
    pub cleanup: Cleanup,
    /// Whether to remove `root` on Drop. The runner flips this to `true` on
    /// a successful run (honored only when `cleanup == OnSuccess`).
    pub(crate) succeeded: std::cell::Cell<bool>,
    /// When the staging dir was created via `tempfile::TempDir`, we hold it
    /// here so it isn't auto-cleaned before our Drop impl runs. Actual
    /// cleanup still goes through the `Drop` logic below so the policy is
    /// honored uniformly across Tmp / Cwd / Home / Custom.
    pub(crate) _temp_dir: Option<tempfile::TempDir>,
}

impl BundleCtx {
    /// Record that the bundle run succeeded; honored on drop when the
    /// cleanup policy is `OnSuccess`.
    pub fn mark_success(&self) {
        self.succeeded.set(true);
    }

    /// Whether the ctx will delete its staging dir on drop, given the
    /// current policy and success flag. Exposed for the CLI so it can print
    /// "left at <path>" when we're going to keep the dir.
    pub fn will_keep_staging(&self) -> bool {
        match self.cleanup {
            Cleanup::Always => false,
            Cleanup::Never => true,
            Cleanup::OnSuccess => !self.succeeded.get(),
        }
    }
}

impl Drop for BundleCtx {
    fn drop(&mut self) {
        let should_delete = !self.will_keep_staging();
        // Take the TempDir out so our policy decides what happens.
        let td = self._temp_dir.take();
        if should_delete {
            // Prefer TempDir::close for the Tmp case (it gives a proper Result);
            // fall back to remove_dir_all for explicit-path stagings.
            if let Some(td) = td {
                let _ = td.close();
            } else if self.root.exists() {
                let _ = std::fs::remove_dir_all(&self.root);
            }
        } else if let Some(td) = td {
            // Keep the tempfile by detaching — its destructor would otherwise
            // delete it even though policy says keep.
            let _ = td.keep();
        }
    }
}

/// Summary returned by [`info`] — used to display a bundle's contents without
/// extracting it.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct BundleInfo {
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub step_count: usize,
    pub bundle_meta: Option<BundleMeta>,
    pub files: Vec<FileEntry>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Matcher for `bundle.binary` globs.
///
/// Wraps a compiled `globset::GlobSet`. An empty matcher (no patterns)
/// returns false for every path, which means "treat everything as templated
/// text" — the default.
#[derive(Debug)]
pub struct BinaryMatcher {
    set: globset::GlobSet,
}

impl Default for BinaryMatcher {
    fn default() -> Self {
        Self {
            set: globset::GlobSet::empty(),
        }
    }
}

impl BinaryMatcher {
    /// Compile `patterns` into a matcher. Invalid globs are surfaced as
    /// `BundleError::Parse`.
    pub fn new(patterns: &[String]) -> Result<Self, BundleError> {
        let mut builder = globset::GlobSetBuilder::new();
        for pat in patterns {
            let glob = globset::Glob::new(pat)
                .map_err(|e| BundleError::Parse(format!("invalid binary glob {pat:?}: {e}")))?;
            builder.add(glob);
        }
        let set = builder
            .build()
            .map_err(|e| BundleError::Parse(format!("failed to build binary globset: {e}")))?;
        Ok(Self { set })
    }

    /// Whether the given bundle-relative path should be copied byte-for-byte.
    pub fn matches(&self, rel_path: &Path) -> bool {
        self.set.is_match(rel_path)
    }
}

// -- Errors --

#[derive(Debug)]
pub enum BundleError {
    Io(std::io::Error),
    MissingManifest,
    PathTraversal(PathBuf),
    NotABundle(PathBuf),
    Parse(String),
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "bundle io error: {e}"),
            Self::MissingManifest => write!(f, "bundle is missing manifest.json or manifest.jsonc at the root"),
            Self::PathTraversal(p) => write!(f, "bundle entry escapes archive root: {}", p.display()),
            Self::NotABundle(p) => write!(f, "not a .rig bundle: {}", p.display()),
            Self::Parse(msg) => write!(f, "bundle parse error: {msg}"),
        }
    }
}

impl From<std::io::Error> for BundleError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}

// -- Public API --

/// Build a `.rig` archive from `src_dir`. The directory must contain a
/// `manifest.json` or `manifest.jsonc` at its root.
///
/// All files under `src_dir` are included, with their paths relative to
/// `src_dir`. Symlinks are stored as symlinks (tar native). Hidden files are
/// included.
pub fn pack(src_dir: &Path, out: &Path) -> Result<(), BundleError> {
    // Verify manifest presence at the root.
    let has_manifest = src_dir.join("manifest.json").is_file()
        || src_dir.join("manifest.jsonc").is_file();
    if !has_manifest {
        return Err(BundleError::MissingManifest);
    }

    // Make parent dir if needed.
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let file = std::fs::File::create(out)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);

    // Walk the tree. `tar::Builder::append_dir_all` does this for us and uses
    // the directory name as the archive prefix. We want archive paths to be
    // *relative to `src_dir`* (no prefix), so we re-create the same behavior
    // with an explicit walk and `append_path_with_name`.
    add_dir_recursive(&mut builder, src_dir, Path::new(""))?;

    builder.finish()?;
    builder.into_inner()?.finish()?;
    Ok(())
}

/// Recursively add `dir`'s contents to `builder` under the archive prefix
/// `archive_prefix` (use `""` for the bundle root).
fn add_dir_recursive(
    builder: &mut tar::Builder<flate2::write::GzEncoder<std::fs::File>>,
    dir: &Path,
    archive_prefix: &Path,
) -> Result<(), BundleError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let archive_path = archive_prefix.join(&name);
        let full_path = entry.path();

        if file_type.is_dir() {
            // Record the directory entry itself (preserves empty dirs in the archive).
            builder.append_path_with_name(&full_path, &archive_path)?;
            add_dir_recursive(builder, &full_path, &archive_path)?;
        } else if file_type.is_symlink() || file_type.is_file() {
            builder.append_path_with_name(&full_path, &archive_path)?;
        }
        // Skip other kinds (sockets, FIFOs) silently.
    }
    Ok(())
}

/// Extract a `.rig` archive into `out_dir`. The output directory is created
/// if it does not exist. Tar entries whose normalized path escapes `out_dir`
/// (absolute paths, leading `..`, etc.) are rejected.
pub fn unpack(archive: &Path, out_dir: &Path) -> Result<(), BundleError> {
    std::fs::create_dir_all(out_dir)?;

    let file = std::fs::File::open(archive)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => BundleError::NotABundle(archive.to_path_buf()),
            _ => BundleError::Io(e),
        })?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(decoder);

    // Don't let the tar crate auto-handle absolute paths. We validate each
    // entry manually and then tell it the final destination.
    ar.set_overwrite(true);
    ar.set_preserve_permissions(true);

    for entry in ar.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.into_owned();

        // Reject traversal: no absolute paths, no `..` segments.
        let safe_rel = sanitize_archive_path(&entry_path)
            .ok_or_else(|| BundleError::PathTraversal(entry_path.clone()))?;

        let target = out_dir.join(&safe_rel);
        // Ensure parent exists.
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }

        entry.unpack(&target)?;
    }
    Ok(())
}

/// Validate an archive entry path: must be relative, must not contain `..`,
/// must not be empty. Returns the cleaned path, or None if the path is
/// unsafe.
fn sanitize_archive_path(p: &Path) -> Option<PathBuf> {
    use std::path::Component;
    if p.as_os_str().is_empty() {
        return None;
    }
    let mut cleaned = PathBuf::new();
    for component in p.components() {
        match component {
            Component::Normal(seg) => cleaned.push(seg),
            Component::CurDir => {}
            // Reject absolute roots and any parent traversal.
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    if cleaned.as_os_str().is_empty() {
        return None;
    }
    Some(cleaned)
}

/// Read a `.rig` archive and return a summary (manifest fields + file list)
/// without fully extracting it.
pub fn info(archive: &Path) -> Result<BundleInfo, BundleError> {
    let mut out = BundleInfo::default();

    // First pass: collect the file list (paths + sizes) and locate the
    // manifest entry.
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut manifest_name: Option<String> = None;

    let f = std::fs::File::open(archive).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => BundleError::NotABundle(archive.to_path_buf()),
        _ => BundleError::Io(e),
    })?;
    let decoder = flate2::read::GzDecoder::new(f);
    let mut ar = tar::Archive::new(decoder);

    for entry in ar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let is_dir = entry.header().entry_type().is_dir();
        let size = entry.header().size().unwrap_or(0);

        let path_str = path.to_string_lossy().into_owned();

        // Capture manifest contents when we hit it. Bundle root only —
        // reject nested `manifest.json` in subdirs.
        if manifest_bytes.is_none() {
            let is_root_manifest = matches!(
                path_str.as_str(),
                "manifest.json" | "manifest.jsonc" | "./manifest.json" | "./manifest.jsonc"
            );
            if is_root_manifest {
                use std::io::Read;
                let mut buf = Vec::with_capacity(size as usize);
                entry.read_to_end(&mut buf)?;
                manifest_bytes = Some(buf);
                manifest_name = Some(path_str.clone());
            }
        }

        out.files.push(FileEntry {
            path: path_str,
            size,
            is_dir,
        });
    }

    let bytes = manifest_bytes.ok_or(BundleError::MissingManifest)?;
    // Strip JSONC comments, then pluck top-level fields.
    let mut stripped = Vec::new();
    std::io::Read::read_to_end(
        &mut json_comments::StripComments::new(bytes.as_slice()),
        &mut stripped,
    )
    .map_err(|e| BundleError::Parse(format!("stripping comments: {e}")))?;

    let value: serde_json::Value = serde_json::from_slice(&stripped)
        .map_err(|e| BundleError::Parse(format!("parsing {}: {e}", manifest_name.as_deref().unwrap_or("manifest"))))?;

    out.name = value.get("name").and_then(|v| v.as_str()).map(str::to_string);
    out.version = value.get("version").and_then(|v| v.as_str()).map(str::to_string);
    out.description = value
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    out.step_count = value
        .get("steps")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    // Bundle section (optional; Task 4 will make this a first-class field on
    // Config, but for `info` we just deserialize it directly).
    if let Some(b) = value.get("bundle") {
        out.bundle_meta = serde_json::from_value(b.clone()).ok();
    }

    Ok(out)
}

/// Open a bundle: extract to a staging directory, parse the manifest, and
/// return both the parsed `Config` and the `BundleCtx` the runner needs.
///
/// The caller owns the returned `BundleCtx`; dropping it triggers the
/// configured cleanup policy (via `TempDir` for the Tmp case, or an explicit
/// cleanup guard for others — handled in Task 9).
pub fn open_bundle(
    archive: &Path,
    cli_vars: &std::collections::HashMap<String, String>,
    placeholder: bool,
) -> Result<(crate::config::Config, BundleCtx), BundleError> {
    // Peek at the manifest to learn where to stage.
    let meta_from_info = info(archive)?;
    let bundle_meta = meta_from_info.bundle_meta.clone().unwrap_or_default();

    // Resolve the staging directory.
    let (root, temp_dir) = resolve_staging_dir(&bundle_meta.extract_to, &meta_from_info)?;

    // Extract the whole archive into the staging dir.
    unpack(archive, &root)?;

    // Parse the manifest as a full rig config (with variable validation etc.).
    let manifest_path = root
        .join("manifest.jsonc")
        .exists()
        .then(|| root.join("manifest.jsonc"))
        .or_else(|| {
            root.join("manifest.json")
                .exists()
                .then(|| root.join("manifest.json"))
        })
        .ok_or(BundleError::MissingManifest)?;

    let manifest_str = manifest_path
        .to_str()
        .ok_or_else(|| BundleError::Parse("non-utf8 manifest path".into()))?;
    let cfg = crate::config::parse_config(manifest_str, cli_vars, placeholder)
        .map_err(|e| BundleError::Parse(e.to_string()))?;

    // Compile the binary glob set from the parsed config (more authoritative
    // than what `info` returned, since parse validates against the schema).
    let binary_patterns = cfg
        .bundle
        .as_ref()
        .map(|b| b.binary.clone())
        .unwrap_or_default();
    let binary = BinaryMatcher::new(&binary_patterns)?;

    let cleanup = cfg
        .bundle
        .as_ref()
        .map(|b| b.cleanup)
        .unwrap_or_default();

    let ctx = BundleCtx {
        root,
        binary,
        cleanup,
        succeeded: std::cell::Cell::new(false),
        _temp_dir: temp_dir,
    };
    Ok((cfg, ctx))
}

/// Resolve the concrete staging directory for a given `extract-to`.
///
/// Returns the path plus an optional `TempDir` handle — the handle is
/// `Some` only for `extract-to: tmp`, where we want auto-cleanup on Drop;
/// for every other option the caller (Task 9) handles cleanup explicitly.
fn resolve_staging_dir(
    extract_to: &ExtractTo,
    info: &BundleInfo,
) -> Result<(PathBuf, Option<tempfile::TempDir>), BundleError> {
    let stem = info
        .name
        .as_deref()
        .unwrap_or("bundle")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>();

    match extract_to {
        ExtractTo::Named(NamedExtractTo::Tmp) => {
            let td = tempfile::Builder::new()
                .prefix(&format!("rig-{stem}-"))
                .tempdir()?;
            let path = td.path().to_path_buf();
            Ok((path, Some(td)))
        }
        ExtractTo::Named(NamedExtractTo::Cwd) => {
            let dir = std::env::current_dir()?.join(format!("rig-{stem}-staging"));
            std::fs::create_dir_all(&dir)?;
            Ok((dir, None))
        }
        ExtractTo::Named(NamedExtractTo::Home) => {
            let home = std::env::var("HOME")
                .map_err(|_| BundleError::Parse("HOME is not set; cannot use extract-to: home".into()))?;
            let dir = PathBuf::from(home).join(format!("rig-{stem}-staging"));
            std::fs::create_dir_all(&dir)?;
            Ok((dir, None))
        }
        ExtractTo::Custom { path } => {
            let expanded = crate::path::expand_tilde(path);
            std::fs::create_dir_all(&expanded)?;
            Ok((expanded, None))
        }
    }
}

/// Detect whether `path` looks like a rig bundle: either the file name ends in
/// `.rig`, or the first two bytes are the gzip magic `1f 8b`.
#[allow(dead_code)] // wired from main.rs in Task 7
pub fn looks_like_bundle(path: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) == Some("rig") {
        return true;
    }
    // Fallback: sniff gzip magic.
    if let Ok(mut f) = std::fs::File::open(path) {
        use std::io::Read;
        let mut buf = [0u8; 2];
        if f.read_exact(&mut buf).is_ok() {
            return buf == [0x1f, 0x8b];
        }
    }
    false
}

/// Returns `true` if `input` looks like a git repo URL (github/gitlab/etc.)
/// rather than a raw JSON/rig file URL. Supports HTTPS and SSH formats:
/// - `https://github.com/user/repo`
/// - `ssh://git@github.com/user/repo`
/// - `git@github.com:user/repo.git`
pub fn looks_like_git_repo(input: &str) -> bool {
    // SSH SCP-style: git@host:owner/repo or git@host:owner/repo.git
    if input.starts_with("git@") && input.contains(':') {
        return true;
    }
    // ssh:// scheme
    if input.starts_with("ssh://") {
        return true;
    }

    if !input.starts_with("http://") && !input.starts_with("https://") {
        return false;
    }
    // If it ends with a known config extension, it's a file URL, not a repo.
    let lower = input.to_lowercase();
    if lower.ends_with(".json")
        || lower.ends_with(".jsonc")
        || lower.ends_with(".rig")
    {
        return false;
    }
    // Match common git hosting patterns: github.com, gitlab.com, bitbucket.org,
    // codeberg.org, or any URL ending in .git
    if lower.ends_with(".git") {
        return true;
    }
    let without_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let host = without_scheme.split('/').next().unwrap_or("");
    let known_hosts = ["github.com", "gitlab.com", "bitbucket.org", "codeberg.org"];
    if known_hosts.contains(&host) {
        // Must have at least owner/repo path segments
        let path_part = without_scheme.strip_prefix(host).unwrap_or("");
        let segments: Vec<&str> = path_part.split('/').filter(|s| !s.is_empty()).collect();
        return segments.len() >= 2;
    }
    false
}

/// Shallow-clone a git repo into a temp directory. Returns the temp dir handle
/// and the path to the cloned repo root.
pub fn clone_repo(url: &str) -> Result<(tempfile::TempDir, PathBuf), BundleError> {
    let td = tempfile::Builder::new()
        .prefix("rig-repo-")
        .tempdir()?;
    let dest = td.path().join("repo");
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", "--single-branch", url])
        .arg(&dest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .map_err(|e| BundleError::Parse(format!("failed to run git: {e}")))?;
    if !status.success() {
        return Err(BundleError::Parse(format!("git clone failed for {url}")));
    }
    Ok((td, dest))
}

/// Open a local directory as a bundle source. Looks for `manifest.jsonc` or
/// `manifest.json` at the root, parses it, and builds a `BundleCtx` pointing
/// at the directory (no extraction needed — it's already on disk).
pub fn open_directory(
    dir: &Path,
    cli_vars: &std::collections::HashMap<String, String>,
    placeholder: bool,
) -> Result<(crate::config::Config, BundleCtx), BundleError> {
    let root = dir.canonicalize().map_err(BundleError::Io)?;
    let manifest_path = if root.join("manifest.jsonc").is_file() {
        root.join("manifest.jsonc")
    } else if root.join("manifest.json").is_file() {
        root.join("manifest.json")
    } else {
        return Err(BundleError::MissingManifest);
    };

    let manifest_str = manifest_path
        .to_str()
        .ok_or_else(|| BundleError::Parse("non-utf8 manifest path".into()))?;
    let cfg = crate::config::parse_config(manifest_str, cli_vars, placeholder)
        .map_err(|e| BundleError::Parse(e.to_string()))?;

    let binary_patterns = cfg
        .bundle
        .as_ref()
        .map(|b| b.binary.clone())
        .unwrap_or_default();
    let binary = BinaryMatcher::new(&binary_patterns)?;
    let cleanup = cfg
        .bundle
        .as_ref()
        .map(|b| b.cleanup)
        .unwrap_or_default();

    let ctx = BundleCtx {
        root,
        binary,
        cleanup,
        succeeded: std::cell::Cell::new(false),
        _temp_dir: None,
    };
    Ok((cfg, ctx))
}

/// Open a cloned git repo as a bundle. Clones the repo, then opens the
/// directory. The `TempDir` handle is stored in the returned `BundleCtx` so
/// cleanup happens on drop.
pub fn open_git_repo(
    url: &str,
    cli_vars: &std::collections::HashMap<String, String>,
    placeholder: bool,
) -> Result<(crate::config::Config, BundleCtx), BundleError> {
    let (td, repo_path) = clone_repo(url)?;
    let root = repo_path.canonicalize().map_err(BundleError::Io)?;
    let manifest_path = if root.join("manifest.jsonc").is_file() {
        root.join("manifest.jsonc")
    } else if root.join("manifest.json").is_file() {
        root.join("manifest.json")
    } else {
        return Err(BundleError::MissingManifest);
    };

    let manifest_str = manifest_path
        .to_str()
        .ok_or_else(|| BundleError::Parse("non-utf8 manifest path".into()))?;
    let cfg = crate::config::parse_config(manifest_str, cli_vars, placeholder)
        .map_err(|e| BundleError::Parse(e.to_string()))?;

    let binary_patterns = cfg
        .bundle
        .as_ref()
        .map(|b| b.binary.clone())
        .unwrap_or_default();
    let binary = BinaryMatcher::new(&binary_patterns)?;
    let cleanup = cfg
        .bundle
        .as_ref()
        .map(|b| b.cleanup)
        .unwrap_or_default();

    let ctx = BundleCtx {
        root,
        binary,
        cleanup,
        succeeded: std::cell::Cell::new(false),
        _temp_dir: Some(td),
    };
    Ok((cfg, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_to_defaults_to_tmp() {
        let et = ExtractTo::default();
        assert!(matches!(et, ExtractTo::Named(NamedExtractTo::Tmp)));
    }

    #[test]
    fn cleanup_defaults_to_on_success() {
        assert_eq!(Cleanup::default(), Cleanup::OnSuccess);
    }

    #[test]
    fn bundle_meta_parses_empty() {
        let meta: BundleMeta = serde_json::from_str("{}").unwrap();
        assert!(matches!(meta.extract_to, ExtractTo::Named(NamedExtractTo::Tmp)));
        assert_eq!(meta.cleanup, Cleanup::OnSuccess);
        assert!(meta.binary.is_empty());
    }

    #[test]
    fn bundle_meta_parses_named_extract_to() {
        let meta: BundleMeta = serde_json::from_str(r#"{"extract-to": "home"}"#).unwrap();
        assert!(matches!(meta.extract_to, ExtractTo::Named(NamedExtractTo::Home)));
    }

    #[test]
    fn bundle_meta_parses_custom_extract_to() {
        let meta: BundleMeta =
            serde_json::from_str(r#"{"extract-to": {"path": "/tmp/x"}}"#).unwrap();
        match meta.extract_to {
            ExtractTo::Custom { path } => assert_eq!(path, "/tmp/x"),
            _ => panic!("expected Custom"),
        }
    }

    #[test]
    fn bundle_meta_parses_cleanup() {
        let meta: BundleMeta = serde_json::from_str(r#"{"cleanup": "always"}"#).unwrap();
        assert_eq!(meta.cleanup, Cleanup::Always);
        let meta: BundleMeta = serde_json::from_str(r#"{"cleanup": "never"}"#).unwrap();
        assert_eq!(meta.cleanup, Cleanup::Never);
        let meta: BundleMeta = serde_json::from_str(r#"{"cleanup": "on-success"}"#).unwrap();
        assert_eq!(meta.cleanup, Cleanup::OnSuccess);
    }

    #[test]
    fn bundle_meta_parses_binary_globs() {
        let meta: BundleMeta =
            serde_json::from_str(r#"{"binary": ["*.png", "assets/**"]}"#).unwrap();
        assert_eq!(meta.binary, vec!["*.png".to_string(), "assets/**".to_string()]);
    }

    #[test]
    fn bundle_meta_rejects_unknown_fields() {
        // `deny_unknown_fields` catches typos like `cleanupz` before they silently no-op.
        let r: Result<BundleMeta, _> = serde_json::from_str(r#"{"cleanupz": "always"}"#);
        assert!(r.is_err());
    }

    #[test]
    fn looks_like_bundle_by_extension() {
        assert!(looks_like_bundle(Path::new("foo.rig")));
        assert!(!looks_like_bundle(Path::new("foo.json")));
    }

    #[test]
    fn binary_matcher_empty_matches_nothing() {
        let m = BinaryMatcher::default();
        assert!(!m.matches(Path::new("x.png")));
        assert!(!m.matches(Path::new("any/path")));
    }

    #[test]
    fn binary_matcher_simple_glob() {
        let m = BinaryMatcher::new(&["*.png".into()]).unwrap();
        assert!(m.matches(Path::new("logo.png")));
        assert!(!m.matches(Path::new("readme.txt")));
    }

    #[test]
    fn binary_matcher_recursive_glob() {
        let m = BinaryMatcher::new(&["assets/**/*.bin".into()]).unwrap();
        assert!(m.matches(Path::new("assets/a.bin")));
        assert!(m.matches(Path::new("assets/nested/deep/b.bin")));
        assert!(!m.matches(Path::new("other/c.bin")));
    }

    #[test]
    fn binary_matcher_rejects_invalid_glob() {
        let err = BinaryMatcher::new(&["[unclosed".into()]).unwrap_err();
        assert!(matches!(err, BundleError::Parse(_)));
    }

    // -- pack / unpack --

    fn write(p: &Path, contents: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    #[test]
    fn pack_requires_manifest_at_root() {
        let src = tempfile::tempdir().unwrap();
        write(&src.path().join("somefile.txt"), "hi");
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("a.rig");
        let err = pack(src.path(), &archive).unwrap_err();
        assert!(matches!(err, BundleError::MissingManifest));
    }

    #[test]
    fn pack_unpack_round_trip_preserves_tree() {
        // Source tree: manifest + a nested file + a file with literal {{braces}}.
        let src = tempfile::tempdir().unwrap();
        write(&src.path().join("manifest.jsonc"), r#"{"name":"t","version":"0.0.1","steps":[]}"#);
        write(&src.path().join("src/hello.txt"), "hello world");
        write(&src.path().join("{{name}}/pyproject.toml"), "[project]\nname = \"x\"\n");

        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("out.rig");
        pack(src.path(), &archive).unwrap();
        assert!(archive.is_file());

        let dst = tempfile::tempdir().unwrap();
        unpack(&archive, dst.path()).unwrap();

        assert_eq!(
            std::fs::read_to_string(dst.path().join("manifest.jsonc")).unwrap(),
            r#"{"name":"t","version":"0.0.1","steps":[]}"#
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("src/hello.txt")).unwrap(),
            "hello world"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("{{name}}/pyproject.toml")).unwrap(),
            "[project]\nname = \"x\"\n"
        );
    }

    #[test]
    fn looks_like_bundle_by_gzip_magic() {
        // An archive without .rig extension should still be detected via magic bytes.
        let src = tempfile::tempdir().unwrap();
        write(&src.path().join("manifest.json"), r#"{"name":"t","version":"0.0.1","steps":[]}"#);
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("out.tgz"); // wrong extension on purpose
        pack(src.path(), &archive).unwrap();
        assert!(looks_like_bundle(&archive));
    }

    #[test]
    fn sanitize_rejects_absolute_paths() {
        assert!(sanitize_archive_path(Path::new("/etc/passwd")).is_none());
    }

    #[test]
    fn sanitize_rejects_parent_dir() {
        assert!(sanitize_archive_path(Path::new("../etc/passwd")).is_none());
        assert!(sanitize_archive_path(Path::new("safe/../../escape")).is_none());
    }

    #[test]
    fn sanitize_strips_current_dir_segments() {
        let cleaned = sanitize_archive_path(Path::new("./a/./b")).unwrap();
        assert_eq!(cleaned, PathBuf::from("a/b"));
    }

    #[test]
    fn sanitize_rejects_empty() {
        assert!(sanitize_archive_path(Path::new("")).is_none());
        assert!(sanitize_archive_path(Path::new(".")).is_none());
    }

    #[test]
    fn unpack_rejects_path_traversal_entry() {
        // Hand-craft a tar.gz archive containing a path-traversal entry by
        // writing the raw 512-byte tar header. The `tar` crate's path-setting
        // methods refuse `..` as defense-in-depth, so we have to construct
        // the on-disk bytes directly to simulate an archive produced by a
        // hostile packer.
        let archive = tempfile::NamedTempFile::new().unwrap();
        {
            use std::io::Write;

            let data: &[u8] = b"gotcha";
            let name = b"../escape.txt";

            let mut header = [0u8; 512];
            // name field: [0..100]
            header[..name.len()].copy_from_slice(name);
            // mode (octal): "0000644\0" at [100..108]
            header[100..108].copy_from_slice(b"0000644\0");
            // uid, gid: "0000000\0"
            header[108..116].copy_from_slice(b"0000000\0");
            header[116..124].copy_from_slice(b"0000000\0");
            // size: octal(6) padded to 11 bytes + NUL
            let size_str = format!("{:011o}\0", data.len());
            header[124..136].copy_from_slice(size_str.as_bytes());
            // mtime: zeros
            header[136..148].copy_from_slice(b"00000000000\0");
            // checksum field: filled with spaces for calculation
            header[148..156].copy_from_slice(b"        ");
            // typeflag: '0' = regular file
            header[156] = b'0';
            // ustar magic + version
            header[257..263].copy_from_slice(b"ustar\0");
            header[263..265].copy_from_slice(b"00");

            // Compute checksum: sum of all bytes with the checksum field as spaces.
            let cksum: u32 = header.iter().map(|b| *b as u32).sum();
            let cksum_str = format!("{cksum:06o}\0 ");
            header[148..156].copy_from_slice(cksum_str.as_bytes());

            let f = std::fs::File::create(archive.path()).unwrap();
            let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            enc.write_all(&header).unwrap();
            enc.write_all(data).unwrap();
            // Pad data to 512-byte block.
            let pad = 512 - (data.len() % 512);
            if pad < 512 {
                enc.write_all(&vec![0u8; pad]).unwrap();
            }
            // Two trailing zero blocks terminate the archive.
            enc.write_all(&[0u8; 1024]).unwrap();
            enc.finish().unwrap();
        }

        let dst = tempfile::tempdir().unwrap();
        let err = unpack(archive.path(), dst.path()).unwrap_err();
        assert!(
            matches!(err, BundleError::PathTraversal(_)),
            "expected PathTraversal, got {err:?}"
        );
        // And the malicious file must not have been written anywhere near
        // dst's parent.
        assert!(!dst.path().parent().unwrap().join("escape.txt").exists());
    }

    #[test]
    fn unpack_creates_out_dir_if_missing() {
        let src = tempfile::tempdir().unwrap();
        write(&src.path().join("manifest.json"), r#"{"name":"t","version":"0.0.1","steps":[]}"#);
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("a.rig");
        pack(src.path(), &archive).unwrap();

        let dst_parent = tempfile::tempdir().unwrap();
        let dst = dst_parent.path().join("nested/deep/dst");
        assert!(!dst.exists());
        unpack(&archive, &dst).unwrap();
        assert!(dst.is_dir());
        assert!(dst.join("manifest.json").is_file());
    }

    // -- info --

    #[test]
    fn info_reads_manifest_and_lists_files() {
        let src = tempfile::tempdir().unwrap();
        write(
            &src.path().join("manifest.jsonc"),
            r#"{
                // comment to make sure jsonc stripping works
                "name": "demo",
                "version": "1.2.3",
                "description": "a demo bundle",
                "bundle": { "cleanup": "always" },
                "steps": [
                    {"name":"a","action":{"kind":"shell","commands":["echo hi"]}},
                    {"name":"b","action":{"kind":"shell","commands":["echo there"]}}
                ]
            }"#,
        );
        write(&src.path().join("src/hello.txt"), "hello");
        let out = tempfile::tempdir().unwrap();
        let archive = out.path().join("demo.rig");
        pack(src.path(), &archive).unwrap();

        let info = info(&archive).unwrap();
        assert_eq!(info.name.as_deref(), Some("demo"));
        assert_eq!(info.version.as_deref(), Some("1.2.3"));
        assert_eq!(info.description.as_deref(), Some("a demo bundle"));
        assert_eq!(info.step_count, 2);
        assert_eq!(info.bundle_meta.map(|m| m.cleanup), Some(Cleanup::Always));
        // Files list should contain at least the manifest and the nested file.
        let file_paths: Vec<&str> = info.files.iter().map(|f| f.path.as_str()).collect();
        assert!(file_paths.iter().any(|p| *p == "manifest.jsonc"));
        assert!(file_paths.iter().any(|p| p.ends_with("hello.txt")));
    }

    #[test]
    fn info_errors_on_missing_manifest() {
        // Build a tar.gz without a manifest at the root.
        use std::io::Write;
        let archive = tempfile::NamedTempFile::new().unwrap();
        {
            let f = std::fs::File::create(archive.path()).unwrap();
            let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            // Zero trailer = empty archive.
            enc.write_all(&[0u8; 1024]).unwrap();
            enc.finish().unwrap();
        }
        let err = info(archive.path()).unwrap_err();
        assert!(matches!(err, BundleError::MissingManifest));
    }

    // -- open_bundle --

    #[test]
    fn open_bundle_extracts_and_parses_manifest() {
        let src = tempfile::tempdir().unwrap();
        write(
            &src.path().join("manifest.jsonc"),
            r#"{
                "name": "opened",
                "version": "0.1.0",
                "bundle": { "extract-to": "tmp", "binary": ["assets/**/*.bin"] },
                "steps": [
                    {"name":"a","action":{"kind":"shell","commands":["echo a"]}}
                ]
            }"#,
        );
        write(&src.path().join("assets/logo.bin"), "BIN");
        write(&src.path().join("text/readme.md"), "hi");

        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("open.rig");
        pack(src.path(), &archive).unwrap();

        let (cfg, ctx) = open_bundle(&archive, &Default::default(), false).unwrap();
        assert_eq!(cfg.name, "opened");
        assert_eq!(cfg.steps.len(), 1);
        assert!(ctx.root.exists());
        assert!(ctx.root.join("manifest.jsonc").is_file());
        assert!(ctx.root.join("assets/logo.bin").is_file());
        // Binary matcher compiled from the manifest's `binary` globs.
        assert!(ctx.binary.matches(Path::new("assets/logo.bin")));
        assert!(!ctx.binary.matches(Path::new("text/readme.md")));
    }

    #[test]
    fn open_bundle_custom_extract_to_honors_path() {
        let src = tempfile::tempdir().unwrap();
        write(
            &src.path().join("manifest.json"),
            r#"{
                "name": "custom",
                "version": "0.1.0",
                "steps": []
            }"#,
        );
        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("custom.rig");
        pack(src.path(), &archive).unwrap();

        // Patch the archive so extract-to: custom points at a known place.
        // Simpler: write a second manifest with the section and re-pack.
        let staging_parent = tempfile::tempdir().unwrap();
        let staging = staging_parent.path().join("my-stage");
        let manifest2 = format!(
            r#"{{
                "name": "custom",
                "version": "0.1.0",
                "bundle": {{ "extract-to": {{ "path": "{}" }} }},
                "steps": []
            }}"#,
            staging.to_str().unwrap().replace('\\', "\\\\")
        );
        write(&src.path().join("manifest.json"), &manifest2);
        pack(src.path(), &archive).unwrap();

        let (_cfg, ctx) = open_bundle(&archive, &Default::default(), false).unwrap();
        assert_eq!(ctx.root, staging);
        assert!(ctx.root.join("manifest.json").is_file());
    }

    // -- cleanup policy --

    fn pack_trivial_bundle(cleanup: &str, extract_to: &str) -> (tempfile::TempDir, PathBuf) {
        let src = tempfile::tempdir().unwrap();
        write(
            &src.path().join("manifest.json"),
            &format!(
                r#"{{
                    "name": "clean",
                    "version": "0.0.1",
                    "bundle": {{ "extract-to": {extract_to}, "cleanup": {cleanup:?} }},
                    "steps": []
                }}"#
            ),
        );
        let archive_dir = tempfile::tempdir().unwrap();
        let archive = archive_dir.path().join("c.rig");
        pack(src.path(), &archive).unwrap();
        (archive_dir, archive)
    }

    #[test]
    fn cleanup_always_removes_on_success() {
        let (_keep_archive, archive) = pack_trivial_bundle("always", r#""tmp""#);
        let (_cfg, ctx) = open_bundle(&archive, &Default::default(), false).unwrap();
        let root = ctx.root.clone();
        assert!(root.exists());
        ctx.mark_success();
        drop(ctx);
        assert!(!root.exists(), "staging dir should be removed");
    }

    #[test]
    fn cleanup_always_removes_on_failure() {
        let (_keep_archive, archive) = pack_trivial_bundle("always", r#""tmp""#);
        let (_cfg, ctx) = open_bundle(&archive, &Default::default(), false).unwrap();
        let root = ctx.root.clone();
        // Do NOT mark_success — simulating a failed run.
        drop(ctx);
        assert!(!root.exists(), "Always policy deletes regardless of success");
    }

    #[test]
    fn cleanup_on_success_keeps_dir_on_failure() {
        let (_keep_archive, archive) = pack_trivial_bundle("on-success", r#""tmp""#);
        let (_cfg, ctx) = open_bundle(&archive, &Default::default(), false).unwrap();
        let root = ctx.root.clone();
        assert!(ctx.will_keep_staging(), "unsucceeded OnSuccess keeps staging");
        drop(ctx);
        assert!(root.exists(), "staging dir kept on failure");
        // Manual cleanup so we don't leak.
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cleanup_on_success_removes_dir_on_success() {
        let (_keep_archive, archive) = pack_trivial_bundle("on-success", r#""tmp""#);
        let (_cfg, ctx) = open_bundle(&archive, &Default::default(), false).unwrap();
        let root = ctx.root.clone();
        ctx.mark_success();
        drop(ctx);
        assert!(!root.exists(), "OnSuccess + success removes staging");
    }

    #[test]
    fn cleanup_never_keeps_dir_even_on_success() {
        let (_keep_archive, archive) = pack_trivial_bundle("never", r#""tmp""#);
        let (_cfg, ctx) = open_bundle(&archive, &Default::default(), false).unwrap();
        let root = ctx.root.clone();
        ctx.mark_success();
        drop(ctx);
        assert!(root.exists(), "Never always keeps staging");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cleanup_always_works_for_custom_extract_to() {
        // Non-Tmp staging still gets cleaned on Always.
        let stage_parent = tempfile::tempdir().unwrap();
        let stage = stage_parent.path().join("my-stage");
        let custom = format!(
            r#"{{"path": "{}"}}"#,
            stage.to_str().unwrap().replace('\\', "\\\\")
        );
        let (_keep_archive, archive) = pack_trivial_bundle("always", &custom);
        let (_cfg, ctx) = open_bundle(&archive, &Default::default(), false).unwrap();
        assert_eq!(ctx.root, stage);
        assert!(stage.exists());
        ctx.mark_success();
        drop(ctx);
        assert!(!stage.exists(), "Custom staging dir should be removed");
    }

    #[test]
    fn looks_like_git_repo_github() {
        assert!(looks_like_git_repo("https://github.com/user/repo"));
        assert!(looks_like_git_repo("https://github.com/user/repo/"));
        assert!(looks_like_git_repo("https://github.com/org/my-setup"));
    }

    #[test]
    fn looks_like_git_repo_dot_git_suffix() {
        assert!(looks_like_git_repo("https://example.com/foo/bar.git"));
        assert!(looks_like_git_repo("https://self-hosted.dev/team/project.git"));
    }

    #[test]
    fn looks_like_git_repo_ssh() {
        assert!(looks_like_git_repo("git@github.com:user/repo.git"));
        assert!(looks_like_git_repo("git@gitlab.com:org/project"));
        assert!(looks_like_git_repo("ssh://git@github.com/user/repo"));
    }

    #[test]
    fn looks_like_git_repo_rejects_file_urls() {
        assert!(!looks_like_git_repo("https://example.com/setup.json"));
        assert!(!looks_like_git_repo("https://example.com/setup.jsonc"));
        assert!(!looks_like_git_repo("https://example.com/setup.rig"));
    }

    #[test]
    fn looks_like_git_repo_rejects_non_urls() {
        assert!(!looks_like_git_repo("./my-dir"));
        assert!(!looks_like_git_repo("/tmp/setup.json"));
        assert!(!looks_like_git_repo("setup.json"));
    }

    #[test]
    fn looks_like_git_repo_rejects_github_without_repo_path() {
        assert!(!looks_like_git_repo("https://github.com/"));
        assert!(!looks_like_git_repo("https://github.com/user"));
    }

    #[test]
    fn open_directory_finds_manifest() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(
            td.path().join("manifest.json"),
            r#"{"name":"test","version":"1.0.0","steps":[]}"#,
        ).unwrap();
        let (cfg, ctx) = open_directory(td.path(), &Default::default(), false).unwrap();
        assert_eq!(cfg.name, "test");
        assert_eq!(ctx.root, td.path().canonicalize().unwrap());
    }

    #[test]
    fn open_directory_errors_without_manifest() {
        let td = tempfile::tempdir().unwrap();
        let result = open_directory(td.path(), &Default::default(), false);
        assert!(result.is_err());
    }
}
