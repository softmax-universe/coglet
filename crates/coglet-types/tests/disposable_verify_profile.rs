//! Disposable dev/test Cargo profile contract.
//!
//! `cargo test` compiles this with `profile.test`. `cargo test --release`
//! compiles it with `profile.release`. Release and bench stay at Cargo defaults,
//! so this file must compile and pass in both modes without requiring
//! debug-assertions or overflow panics.

use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Command;

const CONFIG: &str = include_str!("../../../.cargo/config.toml");

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(index) => &line[..index],
        None => line,
    }
}

/// Assignment tables only. Comment lines are dropped first so a note that
/// mentions `[profile.release]` cannot satisfy or violate the contract.
fn parse_tables(src: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut tables: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for raw in src.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let name = name.trim().to_string();
            assert!(!name.is_empty(), "empty table header");
            tables.entry(name.clone()).or_default();
            current = Some(name);
            continue;
        }
        let table = current
            .as_deref()
            .unwrap_or_else(|| panic!("assignment outside a table: {line}"));
        let (key, value) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("expected key = value, got {line}"));
        tables
            .get_mut(table)
            .expect("current table")
            .insert(key.trim().to_string(), value.trim().to_string());
    }
    tables
}

#[test]
fn disposable_profiles_drop_debug_output_without_release_overrides() {
    let tables = parse_tables(CONFIG);
    for name in ["dev", "test"] {
        let profile = tables
            .get(&format!("profile.{name}"))
            .unwrap_or_else(|| panic!("missing [profile.{name}]"));
        assert_eq!(
            profile.get("debug").map(String::as_str),
            Some("0"),
            "profile.{name} debug must be 0 (no DWARF)"
        );
        assert_eq!(
            profile.get("incremental").map(String::as_str),
            Some("false"),
            "profile.{name} incremental must be false"
        );
        assert_eq!(
            profile.get("debug-assertions").map(String::as_str),
            Some("true"),
            "profile.{name} must keep debug-assertions"
        );
        assert_eq!(
            profile.get("overflow-checks").map(String::as_str),
            Some("true"),
            "profile.{name} must keep overflow-checks"
        );
    }
    assert!(
        !tables.contains_key("profile.release"),
        "do not override [profile.release]; Cargo defaults must stay"
    );
    assert!(
        !tables.contains_key("profile.bench"),
        "do not override [profile.bench]; Cargo defaults must stay"
    );
}

#[cfg(target_os = "linux")]
fn section_names(readelf_stdout: &str) -> Vec<&str> {
    readelf_stdout
        .lines()
        .filter_map(|line| {
            let start = line.find("] ")?;
            let rest = line[start + 2..].trim_start();
            let name = rest.split_whitespace().next()?;
            name.starts_with('.').then_some(name)
        })
        .collect()
}

/// Opt-in artifact gate for Linux workers provisioned with binutils.
/// Ordinary tests must work without readelf and with deliberate symbol overrides.
/// Run this gate explicitly with --ignored --exact compiling_profile_strips_dwarf;
/// a missing/broken tool or a symbol-enabled build must fail, never silently skip.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "explicit Linux disposable-artifact gate; requires readelf and debug=0"]
fn compiling_profile_strips_dwarf() {
    let exe = std::env::current_exe().expect("test executable");
    let names = readelf_sections(&exe);
    assert!(
        names.iter().any(|name| name == ".text"),
        "missing ELF .text section"
    );
    for forbidden in [".debug_info", ".debug_line", ".debug_abbrev"] {
        assert!(
            !names.iter().any(|name| *name == forbidden),
            "{forbidden} present in {}; disposable debug=0 must strip DWARF (sections: {names:?})",
            exe.display()
        );
    }
}

#[cfg(target_os = "linux")]
fn readelf_sections(exe: &Path) -> Vec<String> {
    let output = Command::new("readelf")
        .args(["-S", "--wide"])
        .arg(exe)
        .output()
        .unwrap_or_else(|err| panic!("readelf invocation failed: {err}"));
    assert!(
        output.status.success(),
        "readelf exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    section_names(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn wrapping_add(left: u8, right: u8) -> u8 {
    left + right
}

/// Overflow-checks are independent of debuginfo. The branch selects the
/// compiling profile's Cargo default; it does not assert `cfg!(debug_assertions)`.
#[test]
fn overflow_checks_follow_compiling_profile_not_release_overrides() {
    let exploded = std::panic::catch_unwind(|| wrapping_add(u8::MAX, 1));
    if cfg!(debug_assertions) {
        assert!(
            exploded.is_err(),
            "dev/test must keep overflow-checks on when debug=0"
        );
    } else {
        assert!(
            exploded.is_ok(),
            "release/bench must keep overflow-checks off; do not enable them to satisfy this test"
        );
    }
}
