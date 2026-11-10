//! Thin wrapper around the `git` executable. aWeave M0 keeps every byte of history in git; this module is the
//! only place that spawns it.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct GitOut {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        // Never wait for a human: no editor, no credential prompt.
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no");
    cmd
}

/// Runs git and returns its output whether it succeeded or not.
pub fn run(dir: &Path, args: &[&str]) -> Result<GitOut> {
    let out =
        command(dir, args).stdin(Stdio::null()).output().context("failed to run git (is it installed and on PATH?)")?;
    Ok(GitOut {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Runs git, fails with its stderr when it exits non-zero, returns stdout without the trailing newline.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = run(dir, args)?;
    if !out.ok {
        bail!("git {} failed: {}", args.join(" "), out.stderr.trim());
    }
    Ok(out.stdout.trim_end_matches(['\n', '\r']).to_string())
}

/// Like [`git`], with `input` written to stdin.
pub fn git_stdin(dir: &Path, args: &[&str], input: &str) -> Result<String> {
    let mut child = command(dir, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run git")?;
    child.stdin.take().expect("piped stdin").write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Splits `-z` output into paths.
pub fn split_z(s: &str) -> impl Iterator<Item = &str> {
    s.split('\0').filter(|p| !p.is_empty())
}

/// The top level of the main working tree, also when called from inside a linked worktree.
pub fn discover_root(start: &Path) -> Result<PathBuf> {
    let common = git(start, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .with_context(|| format!("{} is not inside a git repository", start.display()))?;
    let common = PathBuf::from(common);
    if common.file_name().and_then(|n| n.to_str()) != Some(".git") {
        bail!("bare repositories are not supported yet ({})", common.display());
    }
    let root = common.parent().expect(".git has a parent");
    Ok(root.canonicalize().unwrap_or_else(|_| root.to_path_buf()))
}

/// Extra `-c` arguments that give git an identity when the user has none configured (CI, fresh containers).
pub fn ident_args(dir: &Path) -> Vec<&'static str> {
    let has_email = run(dir, &["config", "user.email"]).map(|o| o.ok).unwrap_or(false)
        || std::env::var_os("GIT_COMMITTER_EMAIL").is_some();
    if has_email { vec![] } else { vec!["-c", "user.name=aweave", "-c", "user.email=aweave@localhost"] }
}

/// Files with unresolved conflicts in `dir`.
pub fn conflicted(dir: &Path) -> Vec<String> {
    run(dir, &["diff", "--name-only", "--diff-filter=U", "-z"])
        .map(|o| split_z(&o.stdout).map(str::to_string).collect())
        .unwrap_or_default()
}
