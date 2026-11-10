//! Intent scopes: glob patterns over repository paths.
//!
//! `src/auth/**` and `**/*.sql` are globs (`*` stays inside one directory, `**` crosses them). A plain path
//! without glob characters, such as `src/auth` or `README.md`, matches that file or everything below that
//! directory. `.` means the whole repository.

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

pub struct Scope {
    set: GlobSet,
}

fn has_meta(p: &str) -> bool {
    p.contains(['*', '?', '[', '{'])
}

fn glob(p: &str) -> Result<Glob> {
    GlobBuilder::new(p).literal_separator(true).build().with_context(|| format!("invalid scope pattern `{p}`"))
}

/// Normalizes one pattern: no `./` prefix, no trailing slash, `.` for the whole repository.
pub fn normalize(p: &str) -> String {
    let p = p.trim().replace('\\', "/");
    let p = p.trim_start_matches("./").trim_end_matches('/');
    if p.is_empty() || p == "." { "**".to_string() } else { p.to_string() }
}

/// Normalizes and validates the patterns of a new intent.
pub fn validate(patterns: &[String]) -> Result<Vec<String>> {
    let out: Vec<String> = patterns.iter().map(|p| normalize(p)).collect();
    if out.is_empty() {
        bail!("an intent needs at least one scope pattern (a path or glob, `.` for everything)");
    }
    Scope::new(&out)?;
    Ok(out)
}

impl Scope {
    pub fn new(patterns: &[String]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        for p in patterns {
            let p = normalize(p);
            b.add(glob(&p)?);
            if !has_meta(&p) {
                b.add(glob(&format!("{p}/**"))?);
            }
        }
        Ok(Scope { set: b.build()? })
    }

    pub fn matches(&self, path: &str) -> bool {
        self.set.is_match(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(p: &[&str]) -> Scope {
        Scope::new(&p.iter().map(|x| x.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn plain_paths_cover_directories() {
        let sc = s(&["src/auth"]);
        assert!(sc.matches("src/auth"));
        assert!(sc.matches("src/auth/token.rs"));
        assert!(!sc.matches("src/authz.rs"));
    }

    #[test]
    fn single_star_stays_in_directory() {
        let sc = s(&["src/*.rs"]);
        assert!(sc.matches("src/main.rs"));
        assert!(!sc.matches("src/auth/token.rs"));
        assert!(s(&["src/**/*.rs"]).matches("src/auth/token.rs"));
    }

    #[test]
    fn dot_means_everything() {
        assert!(s(&["."]).matches("a/b/c.txt"));
        assert_eq!(normalize("./src/"), "src");
    }

    #[test]
    fn bad_pattern_is_rejected() {
        assert!(validate(&["src/[".to_string()]).is_err());
        assert!(validate(&[]).is_err());
    }
}
