// Copyright 2026 James Casey
// SPDX-License-Identifier: Apache-2.0

//! Beamtalk UAT harness (BT-2253).
//!
//! Installs a *released* Beamtalk toolchain bundle — the artifact a user
//! actually gets — and exposes it to acceptance scenarios that build and run
//! real `beamtalk.toml` projects. The released bundle is the thing under test;
//! we never build from source here.
//!
//! Toolchain selection (env vars):
//!
//! * `BEAMTALK_UAT_BIN` — path to an already-installed `beamtalk` binary; skips
//!   download entirely (local-dev escape hatch).
//! * `BEAMTALK_UAT_VERSION` — `latest` (default), `nightly`, or an explicit
//!   version like `0.4.0` / `v0.4.0`.
//!
//! Releases are pulled from `jamesc/beamtalk` via `gh release download`, so the
//! exact published asset for the runner's platform is installed and reused
//! across scenarios. See `CLAUDE.md` for the gate philosophy.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// GitHub repo that publishes the Beamtalk toolchain releases.
const RELEASE_REPO: &str = "jamesc/beamtalk";

/// Which released toolchain to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// The latest non-prerelease GitHub release.
    Latest,
    /// The rolling `nightly` pre-release.
    Nightly,
    /// An explicit semver, stored without a leading `v` (e.g. `0.4.0`).
    Exact(String),
}

impl VersionSpec {
    /// Parse a spec string (`latest` / `nightly` / `0.4.0` / `v0.4.0`).
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "" | "latest" => VersionSpec::Latest,
            "nightly" => VersionSpec::Nightly,
            v => VersionSpec::Exact(v.trim_start_matches('v').to_string()),
        }
    }

    /// Read the spec from `BEAMTALK_UAT_VERSION` (default `latest`).
    pub fn from_env() -> Self {
        VersionSpec::parse(&std::env::var("BEAMTALK_UAT_VERSION").unwrap_or_default())
    }

    /// The release tag to download, or `None` to let `gh` pick the latest.
    fn tag(&self) -> Option<String> {
        match self {
            VersionSpec::Latest => None,
            VersionSpec::Nightly => Some("nightly".to_string()),
            VersionSpec::Exact(v) => Some(format!("v{v}")),
        }
    }

    /// Stable directory name for the install cache.
    fn cache_key(&self) -> String {
        match self {
            VersionSpec::Latest => "latest".to_string(),
            VersionSpec::Nightly => "nightly".to_string(),
            VersionSpec::Exact(v) => format!("v{v}"),
        }
    }
}

/// An installed Beamtalk toolchain ready to drive scenarios.
#[derive(Debug, Clone)]
pub struct Toolchain {
    /// Absolute path to the `beamtalk` executable.
    pub bin: PathBuf,
    /// Output of `beamtalk --version` (trimmed).
    pub version: String,
}

impl Toolchain {
    /// Start a `beamtalk` command (add args / `current_dir` at the call site).
    pub fn command(&self) -> Command {
        Command::new(&self.bin)
    }
}

/// The platform triple + archive extension for the current runner, matching the
/// asset naming in `beamtalk`'s `scripts/ci/package-release.sh`.
fn platform() -> Result<(&'static str, &'static str), String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok(("linux-x86_64", "tar.gz")),
        ("macos", "x86_64") => Ok(("macos-x86_64", "tar.gz")),
        ("macos", "aarch64") => Ok(("macos-arm64", "tar.gz")),
        ("windows", "x86_64") => Ok(("windows-x86_64", "zip")),
        (os, arch) => Err(format!("unsupported platform: {os}-{arch}")),
    }
}

fn bin_name() -> &'static str {
    if cfg!(windows) {
        "beamtalk.exe"
    } else {
        "beamtalk"
    }
}

/// Root of this repo (the crate manifest dir).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Install (or reuse a cached install of) the requested toolchain.
///
/// Honours `BEAMTALK_UAT_BIN` as an escape hatch. Otherwise downloads the
/// matching release asset into `.beamtalk-uat/<version>/` and extracts it so
/// `beamtalk` finds its bundled runtime via the standard installed layout.
pub fn install(spec: &VersionSpec) -> Result<Toolchain, String> {
    // Escape hatch: use an already-installed binary as-is.
    if let Ok(bin) = std::env::var("BEAMTALK_UAT_BIN") {
        let bin = PathBuf::from(bin);
        if !bin.exists() {
            return Err(format!("BEAMTALK_UAT_BIN does not exist: {}", bin.display()));
        }
        let version = query_version(&bin)?;
        return Ok(Toolchain { bin, version });
    }

    let prefix = repo_root().join(".beamtalk-uat").join(spec.cache_key());
    let bin = prefix.join("bin").join(bin_name());

    // Nightly/latest are rolling targets — always re-download so we don't test
    // a stale cached install. Exact versions are immutable and safe to reuse.
    let should_download = match spec {
        VersionSpec::Exact(_) => !bin.exists(),
        _ => true,
    };
    if should_download {
        download_and_extract(spec, &prefix)?;
    }
    if !bin.exists() {
        return Err(format!(
            "toolchain install did not produce {} — archive layout may have changed",
            bin.display()
        ));
    }

    let version = query_version(&bin)?;
    Ok(Toolchain { bin, version })
}

/// Process-wide toolchain installed once from `BEAMTALK_UAT_*` env vars.
///
/// Scenarios call this so the (possibly slow) install happens a single time and
/// the bundle is reused across every test in the run.
pub fn shared() -> &'static Toolchain {
    static SHARED: OnceLock<Toolchain> = OnceLock::new();
    SHARED.get_or_init(|| {
        let spec = VersionSpec::from_env();
        install(&spec).unwrap_or_else(|e| panic!("failed to install Beamtalk toolchain: {e}"))
    })
}

/// If an explicit version was requested via env, return it (without leading `v`).
/// `latest`/`nightly` return `None` since the resolved number isn't known ahead.
pub fn requested_exact_version() -> Option<String> {
    match VersionSpec::from_env() {
        VersionSpec::Exact(v) => Some(v),
        _ => None,
    }
}

fn query_version(bin: &Path) -> Result<String, String> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("failed to run `{} --version`: {e}", bin.display()))?;
    if !out.status.success() {
        return Err(format!(
            "`{} --version` exited with {}",
            bin.display(),
            out.status
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn download_and_extract(spec: &VersionSpec, prefix: &Path) -> Result<(), String> {
    let (plat, ext) = platform()?;
    let glob = format!("beamtalk-*-{plat}.{ext}");

    let tmp = repo_root()
        .join(".beamtalk-uat")
        .join(format!("dl-{}-{}", spec.cache_key(), std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("mkdir {}: {e}", tmp.display()))?;

    // `gh release download` resolves "latest" when no tag is given, supports the
    // `nightly` / `vX.Y.Z` tags, and globs assets by platform so we don't have
    // to know the exact version string up front (important for nightly).
    let mut cmd = Command::new("gh");
    cmd.args(["release", "download"]);
    if let Some(tag) = spec.tag() {
        cmd.arg(tag);
    }
    cmd.args(["--repo", RELEASE_REPO])
        .args(["--pattern", &glob])
        .args(["--pattern", &format!("{glob}.sha256")])
        .args(["--dir", &tmp.to_string_lossy()])
        .arg("--clobber");

    let status = cmd
        .status()
        .map_err(|e| format!("failed to spawn `gh release download` (is `gh` installed?): {e}"))?;
    if !status.success() {
        return Err(format!(
            "`gh release download` for {} failed ({status})",
            spec.cache_key()
        ));
    }

    let archive = find_archive(&tmp, ext)?;
    verify_checksum(&archive);

    // Extract into a staging dir, then atomically swap into place so a failed
    // extraction never leaves a half-populated cache entry that we'd reuse.
    let staging = tmp.join("extract");
    std::fs::create_dir_all(&staging).map_err(|e| format!("mkdir {}: {e}", staging.display()))?;
    extract(&archive, &staging, ext)?;

    // Archives carry a top-level `beamtalk-<version>/` directory; promote it.
    let root = single_subdir(&staging)?;
    if let Some(parent) = prefix.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let _ = std::fs::remove_dir_all(prefix);
    std::fs::rename(&root, prefix)
        .map_err(|e| format!("install {} -> {}: {e}", root.display(), prefix.display()))?;

    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

fn find_archive(dir: &Path, ext: &str) -> Result<PathBuf, String> {
    let suffix = format!(".{ext}");
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(&suffix) && !name.ends_with(".sha256") {
            return Ok(path);
        }
    }
    Err(format!(
        "no `*{suffix}` asset downloaded into {} (release may lack this platform)",
        dir.display()
    ))
}

/// Best-effort SHA-256 verification (mirrors `scripts/install.sh`): warn-and-skip
/// if the checksum file or a hashing tool is unavailable.
fn verify_checksum(archive: &Path) {
    let sha_file = PathBuf::from(format!("{}.sha256", archive.display()));
    let Ok(contents) = std::fs::read_to_string(&sha_file) else {
        eprintln!("warning: no checksum file for {}, skipping", archive.display());
        return;
    };
    let Some(expected) = contents.split_whitespace().next() else {
        return;
    };

    let actual = sha256_hex(archive);
    match actual {
        Some(actual) if actual.eq_ignore_ascii_case(expected) => {}
        Some(actual) => panic!(
            "checksum mismatch for {}:\n  expected: {expected}\n  actual:   {actual}",
            archive.display()
        ),
        None => eprintln!("warning: no sha256sum/shasum available, skipping verification"),
    }
}

fn sha256_hex(path: &Path) -> Option<String> {
    for (cmd, args) in [
        ("sha256sum", vec![path.to_string_lossy().to_string()]),
        (
            "shasum",
            vec!["-a".into(), "256".into(), path.to_string_lossy().to_string()],
        ),
    ] {
        if let Ok(out) = Command::new(cmd).args(&args).output() {
            if out.status.success() {
                let line = String::from_utf8_lossy(&out.stdout);
                if let Some(hash) = line.split_whitespace().next() {
                    return Some(hash.to_string());
                }
            }
        }
    }
    None
}

fn extract(archive: &Path, dest: &Path, ext: &str) -> Result<(), String> {
    let status = match ext {
        "tar.gz" => Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status(),
        "zip" => Command::new("unzip")
            .arg("-qo")
            .arg(archive)
            .arg("-d")
            .arg(dest)
            .status(),
        other => return Err(format!("unsupported archive extension: {other}")),
    }
    .map_err(|e| format!("failed to extract {}: {e}", archive.display()))?;
    if !status.success() {
        return Err(format!("extraction of {} failed ({status})", archive.display()));
    }
    Ok(())
}

/// Return the single subdirectory inside `dir` (the archive's top-level
/// `beamtalk-<version>/`), erroring if the layout isn't a single dir.
fn single_subdir(dir: &Path) -> Result<PathBuf, String> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("read_dir {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    match dirs.len() {
        1 => Ok(dirs.pop().unwrap()),
        n => Err(format!(
            "expected one top-level dir in archive, found {n} in {}",
            dir.display()
        )),
    }
}

/// Copy a project from `projects/<name>` (a real `beamtalk new` package) into a
/// fresh temp dir. The returned `StagedProject` auto-cleans on drop so builds
/// don't accumulate in `$TMPDIR`. This is the seed of the general scenario
/// driver (BT-2450).
pub fn stage_project(name: &str) -> StagedProject {
    let src = repo_root().join("projects").join(name);
    assert!(src.is_dir(), "project not found: {}", src.display());

    let dest = std::env::temp_dir().join(format!(
        "beamtalk-uat-{name}-{}-{}",
        std::process::id(),
        next_id()
    ));
    let _ = std::fs::remove_dir_all(&dest);
    copy_dir(&src, &dest).unwrap_or_else(|e| panic!("staging project {name}: {e}"));
    StagedProject(dest)
}

/// A staged project directory that cleans itself up on drop.
pub struct StagedProject(PathBuf);

impl StagedProject {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagedProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl AsRef<Path> for StagedProject {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        // Don't carry build artifacts / VCS metadata into the staged copy.
        if matches!(name.to_str(), Some("_build" | ".git")) {
            continue;
        }
        let from = entry.path();
        let to = dest.join(&name);
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_spec_parsing() {
        assert_eq!(VersionSpec::parse(""), VersionSpec::Latest);
        assert_eq!(VersionSpec::parse("latest"), VersionSpec::Latest);
        assert_eq!(VersionSpec::parse("nightly"), VersionSpec::Nightly);
        assert_eq!(VersionSpec::parse("v0.4.0"), VersionSpec::Exact("0.4.0".into()));
        assert_eq!(VersionSpec::parse("0.4.0"), VersionSpec::Exact("0.4.0".into()));
    }

    #[test]
    fn version_spec_tags() {
        assert_eq!(VersionSpec::Latest.tag(), None);
        assert_eq!(VersionSpec::Nightly.tag(), Some("nightly".into()));
        assert_eq!(VersionSpec::Exact("0.4.0".into()).tag(), Some("v0.4.0".into()));
    }
}
