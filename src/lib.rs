#![warn(rust_2018_idioms)]

use std::env::{self, VarError};
use std::fmt::Display;
use std::path::Path;
use std::process::Command;
use std::str;

use chrono::offset::Utc;
use glob::Pattern;

/// Generates the build metadata constants.
///
/// This is designed to be used in the top-level libraries of npu-tools and generates the following
/// constants:
///
/// * `VERSION`
/// * `GIT_SHORT_HASH`
/// * `BUILD_TIMESTAMP`
#[macro_export]
macro_rules! metadata_constants {
    () => {
        /// The version of the package, as defined in `Cargo.toml`.
        ///
        /// This is automatically set by Cargo using the `CARGO_PKG_VERSION`
        /// environment variable.
        pub const VERSION: &str = env!("CARGO_PKG_VERSION");

        /// The short hash of the current Git commit at build time.
        ///
        /// This is set via the `FURIOSA_GIT_SHORT_HASH` environment variable,
        /// which is typically defined in a build script (`build.rs`) by calling
        /// `furiosa_metadata::set_metadata_env_vars()`.
        pub const GIT_SHORT_HASH: &str = env!("FURIOSA_GIT_SHORT_HASH");

        /// The full hash of the current Git commit at build time.
        ///
        /// This is set via the `FURIOSA_GIT_FULL_HASH` environment variable,
        /// which is typically defined in a build script (`build.rs`) by calling
        /// `furiosa_metadata::set_metadata_env_vars()`.
        pub const GIT_FULL_HASH: &str = env!("FURIOSA_GIT_FULL_HASH");

        /// The timestamp when the build was created.
        ///
        /// This is set via the `FURIOSA_BUILD_TIMESTAMP` environment variable,
        /// which is typically defined in a build script (`build.rs`) by calling
        /// `furiosa_metadata::set_metadata_env_vars()`.
        pub const BUILD_TIMESTAMP: &str = env!("FURIOSA_BUILD_TIMESTAMP");
    };
}

/// Sets the build metadata environment variables.
///
/// This is designed to be used as a part of a Cargo build script and sets the following
/// environment variables:
///
/// * `FURIOSA_GIT_SHORT_HASH`
/// * `FURIOSA_BUILD_TIMESTAMP`
///
/// Following environment variables may be used for configuration:
///
/// * `FURIOSA_METADATA_EXPECT_MODIFIED` is a colon-separated list of glob patterns
///   that are ignored for the dirty repository detection (puts `-modified` to the hash).
///   Patterns match the full path, so `*.bak` doesn't match `foo/bar.bak` (`**/*.bak` does).
///   See the `glob` crate documentation for the full pattern syntax.
pub fn set_metadata_env_vars() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(VarError::NotPresent) = env::var("FURIOSA_GIT_SHORT_HASH") {
        let expected_patterns = get_expected_patterns()?;
        println!("cargo:rustc-env=FURIOSA_GIT_SHORT_HASH={}", git_hash(&expected_patterns, true)?);
    }

    if let Err(VarError::NotPresent) = env::var("FURIOSA_GIT_FULL_HASH") {
        let expected_patterns = get_expected_patterns()?;
        println!("cargo:rustc-env=FURIOSA_GIT_FULL_HASH={}", git_hash(&expected_patterns, false)?);
    }

    println!("cargo:rustc-env=FURIOSA_BUILD_TIMESTAMP={}", build_timestamp());

    Ok(())
}

fn get_expected_patterns() -> Result<Vec<Pattern>, Box<dyn std::error::Error>> {
    const PATTERN_VAR: &str = "FURIOSA_METADATA_EXPECT_MODIFIED";

    println!("cargo:rerun-if-env-changed={PATTERN_VAR}");
    match env::var(PATTERN_VAR) {
        Ok(patterns) if patterns.is_empty() => Ok(vec![]),
        Ok(patterns) => patterns
            .split(':')
            .map(|pattern| {
                if pattern.is_empty() {
                    Err(format!("{PATTERN_VAR} contains an empty pattern").into())
                } else {
                    Pattern::new(pattern).map_err(|e| {
                        format!("{PATTERN_VAR} contains an invalid pattern {pattern:?}: {e}").into()
                    })
                }
            })
            .collect(),
        Err(VarError::NotPresent) => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

/// Returns the Git short hash for the current branch of the npu-tools repository.
///
/// The hash will have a `-modified` suffix if the repository is dirty.
/// A repository is considered clean if all updated paths (if any) match any `expected_patterns`.
fn git_hash(expected_patterns: &[Pattern], short: bool) -> Result<String, Box<dyn std::error::Error>> {
    let args: &[&str] = if short {
        &["rev-parse", "--short=9", "HEAD"] // guarantee at least 9 letters, for backward compatibility
    } else {
        &["rev-parse", "HEAD"]
    };
    let mut git_hash = run_git(
        &args,
        |s| {
            let s = s.trim_end();
            if s.len() >= 9 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
                Ok(s.to_owned())
            } else {
                Err("bad commit id")
            }
        },
    )?;

    let dirty = run_git(
        &[
            "status",
            "--untracked=no",          // ignore untracked files (`??`)
            "--ignore-submodules=all", // ignore all submodule changes
            "--no-renames",            // do not detect renames
            "--porcelain",             // use the machine-readable format
            "-z",                      // all paths are zero-terminated
        ],
        |s| {
            // https://git-scm.com/docs/git-status#_porcelain_format_version_1
            // We can safely assume that the whole output consists of `XY <name>\0`
            // because `--no-renames` prohibits `XY <new name>\0<old name>\0`.
            let mut dirty = false;
            for line in s.split_terminator('\0') {
                if line.starts_with("?? ") {
                    return Err("untracked file should have been omitted");
                }
                if line.starts_with("!! ") {
                    return Err("ignored file should have been omitted");
                }
                if !matches!(
                    line.as_bytes(),
                    [
                        b' ' | b'M' | b'T' | b'A' | b'D' | b'R' | b'C' | b'U',
                        b' ' | b'M' | b'T' | b'A' | b'D' | b'R' | b'C' | b'U',
                        b' ',
                        _,
                        ..
                    ]
                ) {
                    return Err("bad status");
                }

                let path = &line[3..];
                if expected_patterns.iter().any(|pattern| pattern.matches(path)) {
                    eprintln!(
                        "[furiosa-metadata] Ignored an updated file {path:?} as it was expected."
                    );
                } else {
                    dirty = true;
                }
            }
            Ok(dirty)
        },
    )?;

    if dirty {
        git_hash.push_str("-modified");
    }

    Ok(git_hash)
}

fn extract_stdout<'a>(
    cmd_line: &'_ str,
    output: &'a std::process::Output,
) -> Result<&'a str, String> {
    if !output.status.success() {
        return Err(format!(
            "`{cmd_line}` failed: {status}\n\n{stderr}",
            status = output.status,
            stderr = output.stderr.escape_ascii(),
        ));
    }

    let stdout = str::from_utf8(&output.stdout).map_err(|e| {
        format!(
            "Unexpected output from `{cmd_line}`: {e}\n\n{stdout}",
            stdout = output.stdout.escape_ascii(),
        )
    })?;

    Ok(stdout)
}

fn get_workspace_dir() -> Result<String, Box<dyn std::error::Error>> {
    let command = env!("CARGO");
    let args = ["locate-project", "--workspace", "--message-format=plain"];
    let output = Command::new(command).args(args).output()?;

    let cmd_line: String = format!("{command} {}", args.join(" "));
    let stdout = extract_stdout(&cmd_line, &output)?;

    let cargo_path = Path::new(stdout.trim());
    Ok(cargo_path.parent().unwrap().display().to_string())
}

/// Run git with given arguments, as if it was run from the workspace directory,
/// and try to parse the resulting stdout with given function.
/// Returns a formatted error with stdout or stderr on any error.
fn run_git<T, E: Display>(
    args: &[&str],
    parse: impl Fn(&str) -> Result<T, E>,
) -> Result<T, Box<dyn std::error::Error>> {
    let workspace_dir: String = get_workspace_dir()?;

    let cmd_line = format!("git -C {workspace_dir} {args}", args = args.join(" "));
    let output = Command::new("git").args(["-C", &workspace_dir]).args(args).output()?;
    let stdout = extract_stdout(&cmd_line, &output)?;

    Ok(parse(stdout)
        .map_err(|e| format!("Unexpected output from `{cmd_line}`: {e}\n\n{stdout}"))?)
}

/// Returns the date and time of the current build.
fn build_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[test]
fn tests() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!git_short_hash(&[])?.is_empty());
    Ok(())
}
