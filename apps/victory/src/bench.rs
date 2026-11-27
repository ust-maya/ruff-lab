//! `victory bench`: benchmark skeleton. Task 1 only wires the CLI and the
//! `--set KEY=VALUE` parser; the actual measurement lands later.

use std::path::{Path, PathBuf};

use anyhow::Result;

#[cfg(windows)]
pub fn run(_exe: &Path, _frames: u32, _out: Option<PathBuf>, _set: Vec<String>) -> Result<()> {
    anyhow::bail!("benchmarking not implemented yet")
}

#[cfg(not(windows))]
pub fn run(_: &Path, _: u32, _: Option<PathBuf>, _: Vec<String>) -> Result<()> {
    anyhow::bail!("running titles requires Windows")
}

/// Parse one `--set KEY=VALUE` argument into its key and value.
/// Only the tests use it for now; the bench run wires it up later.
#[allow(dead_code)]
pub fn parse_set_arg(s: &str) -> Result<(String, String)> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("expected KEY=VALUE, got {s:?}"))?;
    anyhow::ensure!(!key.is_empty(), "expected KEY=VALUE, got {s:?}");
    Ok((key.to_owned(), value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::parse_set_arg;

    #[test]
    fn parse_set_arg_parses_key_and_value() {
        assert_eq!(
            parse_set_arg("A=1").unwrap(),
            ("A".to_owned(), "1".to_owned())
        );
        assert!(parse_set_arg("NOEQ").is_err());
        assert!(parse_set_arg("=x").is_err());
    }
}
