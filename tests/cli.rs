//! CLI integration tests (PLAN.md §14.2): self-spawn with piped stdio, no pty.
//!
//! The binary under test is located via `CARGO_BIN_EXE_daylight`, the path
//! cargo provides to integration tests of a package with a `[[bin]]` target.
//! (PLAN §14.2 says `std::env::current_exe()` — written when tests were to
//! live inline in `src/main.rs`; from `tests/`, `current_exe()` is the test
//! harness itself, so `CARGO_BIN_EXE_daylight` is the faithful equivalent.)
//!
//! Piped stdout is not a TTY, which implies one-shot mode (PLAN §9 "Non-TTY
//! stdout"): every plain run renders exactly one frame and exits 0.

use std::io::Read;
use std::process::{Command, Stdio};

mod common;

/// Spawn the built binary with `args`; stdin is piped and its write end
/// closed immediately (EOF), stdout/stderr are piped and drained
/// concurrently. Returns (stdout bytes, stderr bytes, exit code).
fn spawn(args: &[&str]) -> (Vec<u8>, Vec<u8>, i32) {
    spawn_env(args, &[])
}

/// [`spawn`] with extra environment variables layered over the inherited set.
fn spawn_env(args: &[&str], envs: &[(&str, &str)]) -> (Vec<u8>, Vec<u8>, i32) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_daylight"));
    cmd.args(args).envs(envs.iter().copied());
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn the daylight binary");
    drop(child.stdin.take()); // immediate EOF; the app must not depend on stdin

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let mut err_buf = Vec::new();
    let _ = stderr.read_to_end(&mut err_buf);
    let out_buf = reader.join().expect("stdout reader panicked");

    let status = child.wait().expect("waiting for the daylight binary");
    (out_buf, err_buf, status.code().unwrap_or(-1))
}

/// True if `bytes` contains any braille pattern char (U+2800..=U+28FF,
/// UTF-8: E2 A0..A3 80..BF).
fn has_braille(bytes: &[u8]) -> bool {
    common::has_braille(bytes)
}

#[test]
fn help_exits_zero_and_documents_flags() {
    let (out, err, code) = spawn(&["--help"]);
    assert_eq!(code, 0, "--help must exit 0");
    assert!(err.is_empty(), "--help must not write to stderr: {err:?}");
    let help = String::from_utf8(out).expect("help output is UTF-8");
    assert!(
        help.contains("Usage:"),
        "help must contain a usage line:\n{help}"
    );
    for flag in [
        "--center",
        "--utc",
        "--twilight",
        "--ascii",
        "--color",
        "--once",
    ] {
        assert!(help.contains(flag), "help must document {flag}:\n{help}");
    }
}

#[test]
fn help_documents_interactive_keys() {
    // PLAN §12: "--help documents them" — the interactive keys
    // (q, Esc, Ctrl-C quit · Ctrl-Z suspend · u/c/t/r actions).
    let (out, _err, code) = spawn(&["--help"]);
    assert_eq!(code, 0);
    let help = String::from_utf8(out).expect("help output is UTF-8");
    let lower = help.to_lowercase();
    let start = match lower.find("interactive") {
        Some(i) => i,
        None => panic!("--help must document the interactive keys (PLAN §12); it said:\n{help}"),
    };
    // A window starting at the keys section, so flag descriptions elsewhere
    // in the help cannot satisfy the key checks by accident.
    let window = &lower[start..lower.len().min(start + 500)];
    assert!(
        ["ctrl-z", "ctrl+z", "^z"]
            .iter()
            .any(|n| window.contains(n)),
        "--help must document the Ctrl-Z (suspend) key; keys section was:\n{window}"
    );
    for needle in ["ctrl-c", "esc"] {
        assert!(
            window.contains(needle),
            "--help must document the {needle} quit key; keys section was:\n{window}"
        );
    }
    for key in ["q", "u", "c", "t", "o", "a", "r"] {
        let listed = window.contains(&format!("`{key}`"))
            || window.contains(&format!("{key},"))
            || window.contains(&format!(" {key} "));
        assert!(
            listed,
            "--help must list the '{key}' interactive key; keys section was:\n{window}"
        );
    }
}

#[test]
fn version_prints_semver_line() {
    let (out, err, code) = spawn(&["--version"]);
    assert_eq!(code, 0, "--version must exit 0");
    assert!(
        err.is_empty(),
        "--version must not write to stderr: {err:?}"
    );
    let line = String::from_utf8(out).expect("version output is UTF-8");
    let line = line.trim();
    let expect = format!("daylight {}", env!("CARGO_PKG_VERSION"));
    assert_eq!(line, expect, "--version must print `daylight <semver>`");
}

#[test]
fn center_out_of_range_is_usage_error() {
    let (out, err, code) = spawn(&["--center", "999"]);
    assert_eq!(code, 2, "usage errors must exit 2 (clap convention)");
    assert!(!err.is_empty(), "the clap error must go to stderr");
    assert!(
        out.is_empty(),
        "usage errors must not render a frame on stdout"
    );
}

#[test]
fn center_boundaries_are_accepted() {
    // The documented range is inclusive: both dateline meridians are valid.
    for center in ["-180", "180"] {
        let (out, err, code) = spawn(&["--center", center, "--once"]);
        assert_eq!(code, 0, "--center {center} must be accepted");
        assert!(err.is_empty(), "nothing on stderr: {err:?}");
        assert!(has_braille(&out), "a frame must be rendered");
    }
}

#[test]
fn interval_bounds_are_validated() {
    // 0 would busy-spin the rotating loop; > 1000 was silently clamped —
    // both are usage errors now, with the range documented in --help.
    for bad in ["0", "9", "1001", "60000"] {
        let (out, err, code) = spawn(&["--interval", bad]);
        assert_eq!(code, 2, "--interval {bad} must be a usage error");
        assert!(!err.is_empty(), "the clap error must go to stderr");
        assert!(out.is_empty(), "no frame on stdout");
        assert!(
            String::from_utf8_lossy(&err).contains("10, 1000"),
            "error should state the valid range: {err:?}"
        );
    }
    // The inclusive bounds themselves are accepted.
    for good in ["10", "100", "1000"] {
        let (out, _err, code) = spawn(&["--interval", good, "--once"]);
        assert_eq!(code, 0, "--interval {good} must be accepted");
        assert!(has_braille(&out));
    }
}

#[test]
fn no_color_does_not_override_explicit_always() {
    // `--color always` beats NO_COLOR by documented precedence.
    let (out, _err, code) = spawn_env(&["--color", "always", "--once"], &[("NO_COLOR", "1")]);
    assert_eq!(code, 0);
    assert!(out.contains(&0x1b), "--color always must win over NO_COLOR");
}

#[test]
fn piped_stdout_renders_braille_frame() {
    // Piped stdout implies one-shot mode: one frame, exit 0 (PLAN §9).
    let (out, err, code) = spawn(&[]);
    assert_eq!(code, 0);
    assert!(err.is_empty(), "nothing extraneous on stderr: {err:?}");
    assert!(!out.is_empty(), "a frame must be rendered");
    assert!(has_braille(&out), "the frame must contain braille glyphs");
}

#[test]
fn ascii_flag_renders_hash_land_without_braille() {
    let (out, _err, code) = spawn(&["--ascii", "--once"]);
    assert_eq!(code, 0);
    let text = String::from_utf8(out.clone()).expect("frame is UTF-8");
    assert!(text.contains('#'), "--ascii must mark land with '#'");
    assert!(!has_braille(&out), "--ascii must not emit braille");
}

#[test]
fn color_modes_in_one_shot() {
    // `--color always`: SGR escape sequences present.
    let (out, _err, code) = spawn(&["--color", "always", "--once"]);
    assert_eq!(code, 0);
    assert!(
        out.contains(&0x1b),
        "--color always must emit SGR (ESC) sequences"
    );

    // `--color never`: byte-for-byte free of escapes.
    let (out, _err, code) = spawn(&["--color", "never", "--once"]);
    assert_eq!(code, 0);
    assert!(
        !out.contains(&0x1b),
        "--color never must not emit any ESC byte"
    );

    // `auto` with piped (non-TTY) stdout also resolves to no color (§11).
    let (out, _err, code) = spawn(&["--color", "auto", "--once"]);
    assert_eq!(code, 0);
    assert!(
        !out.contains(&0x1b),
        "--color auto on a non-TTY must not emit ESC bytes"
    );
}

#[test]
fn utc_clock_has_no_offset_suffix() {
    // The clock line format is e.g. "… UTC+02:00"; with --utc the zone must
    // be bare "UTC" (offset 0 collapses the suffix — see frame::clock_line).
    let (out, _err, code) = spawn(&["--once", "--utc"]);
    assert_eq!(code, 0);
    let text = String::from_utf8(out).expect("frame is UTF-8");
    let clock = text
        .lines()
        .rev()
        .find(|l| l.contains("UTC"))
        .unwrap_or_else(|| panic!("no clock line containing UTC in output:\n{text}"));
    assert!(
        clock.contains("· ☉"),
        "the clock line should carry the subsolar readout: {clock}"
    );
    assert!(
        !clock.contains("UTC+") && !clock.contains("UTC-"),
        "--utc must display a bare UTC zone, got: {clock}"
    );
}
