#![warn(rust_2018_idioms)]

use std::env::{self, VarError};
use std::fmt::Display;
use std::process::Command;
use std::str;

use chrono::offset::Utc;

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
        pub const VERSION: &str = env!("CARGO_PKG_VERSION");
        pub const GIT_SHORT_HASH: &str = env!("FURIOSA_GIT_SHORT_HASH");
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
pub fn set_metadata_env_vars() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(VarError::NotPresent) = env::var("FURIOSA_GIT_SHORT_HASH") {
        println!("cargo:rustc-env=FURIOSA_GIT_SHORT_HASH={}", git_short_hash()?);
    }

    println!("cargo:rustc-env=FURIOSA_BUILD_TIMESTAMP={}", build_timestamp());

    Ok(())
}

/// Returns the Git short hash for the current branch of the npu-tools repository.
///
/// The hash will have a `-modified` suffix if the repository is dirty.
fn git_short_hash() -> Result<String, Box<dyn std::error::Error>> {
    let mut git_short_hash = run_git(
        &[
            "rev-parse",
            "--short=9", // guarantee at least 9 letters, for backward compatibility
            "HEAD",
        ],
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
            "--porcelain",             // use the machine-readable format
                                       // (we don't use file names, so don't need `-z`)
        ],
        |s| {
            // https://git-scm.com/docs/git-status#_porcelain_format_version_1
            let mut dirty = false;
            for line in s.lines() {
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
                dirty = true;
            }
            Ok(dirty)
        },
    )?;

    if dirty {
        git_short_hash.push_str("-modified");
    }

    Ok(git_short_hash)
}

/// Run git with given arguments, as if it was run from the workspace directory,
/// and try to parse the resulting stdout with given function.
/// Returns a formatted error with stdout or stderr on any error.
fn run_git<T, E: Display>(
    args: &[&str],
    parse: impl Fn(&str) -> Result<T, E>,
) -> Result<T, Box<dyn std::error::Error>> {
    const WORKSPACE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    let command = || format!("git -C {WORKSPACE_DIR} {args}", args = args.join(" "));
    let output = Command::new("git").args(["-C", WORKSPACE_DIR]).args(args).output()?;

    if !output.status.success() {
        return Err(format!(
            "`{command}` failed: {status}\n\n{stderr}",
            command = command(),
            status = output.status,
            stderr = output.stderr.escape_ascii(),
        )
        .into());
    }

    let stdout = str::from_utf8(&output.stdout).map_err(|e| {
        format!(
            "Unexpected output from `{command}`: {e}\n\n{stdout}",
            command = command(),
            stdout = output.stdout.escape_ascii(),
        )
    })?;

    Ok(parse(stdout).map_err(|e| {
        format!("Unexpected output from `{command}`: {e}\n\n{stdout}", command = command())
    })?)
}

/// Returns the date and time of the current build.
fn build_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}