//! Parser combinators for cargo fingerprint log analysis
//!
//! This module uses the nom parser combinator library to parse cargo's
//! fingerprint log output and extract structured rebuild reasons.

use nom::{
    IResult,
    branch::alt,
    bytes::complete::{tag, take_until},
    character::complete::{char, digit1, space0},
    combinator::map,
    error::Error,
    sequence::{delimited, preceded, terminated, tuple},
};

use crate::{rebuild_graph::PackageTarget, rebuild_reason::RebuildReason};

/// A parsed rebuild entry with package context and reason
#[derive(Debug, Clone)]
pub struct ParsedRebuildEntry {
    pub package: PackageTarget,
    pub reason: RebuildReason,
}

impl ParsedRebuildEntry {
    #[must_use]
    pub const fn new(package: PackageTarget, reason: RebuildReason) -> Self {
        Self { package, reason }
    }
}

/// Extract package context from cargo log line
/// Parses patterns like: `prepare_target{force=false package_id=libz-sys
/// v1.1.23 target="build-script-build"}`
fn extract_package_context(line: &str) -> PackageTarget {
    let package_id = line.find("package_id=").map_or_else(
        || "unknown".to_string(),
        |pkg_start| {
            let after_pkg = &line[pkg_start + 11..];
            let end = after_pkg
                .find(" target=")
                .or_else(|| after_pkg.find('}'))
                .unwrap_or(after_pkg.len());
            after_pkg[..end].trim().to_string()
        },
    );

    let target = line.find("target=").and_then(|target_start| {
        let after_target = &line[target_start + 7..];

        if let Some(stripped) = after_target.strip_prefix('"') {
            return stripped
                .find('"')
                .map(|quote_end| stripped[..quote_end].to_string());
        }

        let end = after_target
            .find([' ', '}', ':'])
            .unwrap_or(after_target.len());
        let value = after_target[..end].trim();

        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    });

    PackageTarget::new(package_id, target)
}

// Parse a quoted string: "hello world"
fn parse_quoted_string(input: &str) -> IResult<&str, String> {
    delimited(
        char('"'),
        map(take_until("\""), |s: &str| s.to_string()),
        char('"'),
    )(input)
}

// Parse a number (used for fingerprints)
fn parse_number(input: &str) -> IResult<&str, String> {
    map(digit1, |s: &str| s.to_string())(input)
}

// Parse Option<T>: "Some(value)" or "None"
fn parse_option_string(input: &str) -> IResult<&str, Option<String>> {
    alt((
        map(tag("None"), |_| None),
        map(
            preceded(tag("Some("), terminated(parse_quoted_string, char(')'))),
            Some,
        ),
    ))(input)
}

// Parse field assignment: field_name: value (simplified helper functions)

// Parse comma separator with optional whitespace
fn parse_comma(input: &str) -> IResult<&str, ()> {
    map(tuple((space0, char(','), space0)), |_| ())(input)
}

// Parse EnvVarChanged { name: "CC", old_value: Some("gcc"), new_value: None }
fn parse_env_var_changed(input: &str) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("EnvVarChanged")(input)?;
    let (input, _) = tuple((space0, char('{'), space0))(input)?;

    // Parse name: "value"
    let (input, _) = tuple((tag("name"), space0, char(':'), space0))(input)?;
    let (input, name) = parse_quoted_string(input)?;
    let (input, ()) = parse_comma(input)?;

    // Parse old_value: Option<String>
    let (input, _) = tuple((tag("old_value"), space0, char(':'), space0))(input)?;
    let (input, old_value) = parse_option_string(input)?;
    let (input, ()) = parse_comma(input)?;

    // Parse new_value: Option<String>
    let (input, _) = tuple((tag("new_value"), space0, char(':'), space0))(input)?;
    let (input, new_value) = parse_option_string(input)?;

    let (input, _) = tuple((space0, char('}')))(input)?;

    Ok((
        input,
        RebuildReason::EnvVarChanged {
            name,
            old_value,
            new_value,
        },
    ))
}

// Parse UnitDependencyInfoChanged { old_name: "rusqlite", old_fingerprint: 123,
// new_name: "rusqlite", new_fingerprint: 456 }
fn parse_unit_dependency_info_changed(input: &str) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("UnitDependencyInfoChanged")(input)?;
    let (input, _) = tuple((space0, char('{'), space0))(input)?;

    // Parse old_name: "value"
    let (input, _) = tuple((tag("old_name"), space0, char(':'), space0))(input)?;
    let (input, old_name) = parse_quoted_string(input)?;
    let (input, ()) = parse_comma(input)?;

    // Parse old_fingerprint: number
    let (input, _) = tuple((tag("old_fingerprint"), space0, char(':'), space0))(input)?;
    let (input, old_fingerprint) = parse_number(input)?;
    let (input, ()) = parse_comma(input)?;

    // Parse new_name: "value" (but we don't need to store it)
    let (input, _) = tuple((tag("new_name"), space0, char(':'), space0))(input)?;
    let (input, _) = parse_quoted_string(input)?;
    let (input, ()) = parse_comma(input)?;

    // Parse new_fingerprint: number
    let (input, _) = tuple((tag("new_fingerprint"), space0, char(':'), space0))(input)?;
    let (input, new_fingerprint) = parse_number(input)?;

    let (input, _) = tuple((space0, char('}')))(input)?;

    Ok((
        input,
        RebuildReason::UnitDependencyInfoChanged {
            name: old_name,
            old_fingerprint,
            new_fingerprint,
            context: None, // We'll enhance this with context parsing later
        },
    ))
}

// Parse TargetConfigurationChanged
fn parse_target_configuration_changed(input: &str) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("TargetConfigurationChanged")(input)?;
    Ok((input, RebuildReason::TargetConfigurationChanged))
}

// Parse ProfileConfigurationChanged
fn parse_profile_configuration_changed(input: &str) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("ProfileConfigurationChanged")(input)?;
    Ok((input, RebuildReason::ProfileConfigurationChanged))
}

// Parse a Vec<String>: [elem1, elem2, ...]
fn parse_string_vec(input: &str) -> IResult<&str, Vec<String>> {
    let (input, _) = char('[')(input)?;
    let (input, _) = space0(input)?;

    let mut result = Vec::new();
    let mut input = input;

    loop {
        input = space0(input)?.0;

        if let Ok((rest, _)) = char::<&str, Error<&str>>(']')(input) {
            return Ok((rest, result));
        }

        let (rest, s) = parse_quoted_string(input)?;
        result.push(s);
        input = rest;

        input = space0(input)?.0;

        if let Ok((rest, _)) = char::<&str, Error<&str>>(']')(input) {
            return Ok((rest, result));
        }

        let (rest, _) = char(',')(input)?;
        input = rest;
    }
}

// Parse RustflagsChanged { old: [...], new: [...] }
fn parse_rustflags_changed(input: &str) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("RustflagsChanged")(input)?;
    let (input, _) = tuple((space0, char('{'), space0))(input)?;

    let (input, _) = tuple((tag("old"), space0, char(':'), space0))(input)?;
    let (input, old) = parse_string_vec(input)?;
    let (input, ()) = parse_comma(input)?;

    let (input, _) = tuple((tag("new"), space0, char(':'), space0))(input)?;
    let (input, new) = parse_string_vec(input)?;

    let (input, _) = tuple((space0, char('}')))(input)?;

    Ok((input, RebuildReason::RustflagsChanged { old, new }))
}

// Parse FeaturesChanged { old: "...", new: "..." }
fn parse_features_changed(input: &str) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("FeaturesChanged")(input)?;
    let (input, _) = tuple((space0, char('{'), space0))(input)?;

    let (input, _) = tuple((tag("old"), space0, char(':'), space0))(input)?;
    let (input, old) = parse_quoted_string(input)?;
    let (input, ()) = parse_comma(input)?;

    let (input, _) = tuple((tag("new"), space0, char(':'), space0))(input)?;
    let (input, new) = parse_quoted_string(input)?;

    let (input, _) = tuple((space0, char('}')))(input)?;

    Ok((input, RebuildReason::FeaturesChanged { old, new }))
}

// Parse FileTime { seconds: 123, nanos: 456 }
fn parse_file_time(input: &str) -> IResult<&str, (String, String)> {
    let (input, _) = tag("FileTime")(input)?;
    let (input, _) = tuple((space0, char('{'), space0))(input)?;

    // Parse seconds: number
    let (input, _) = tuple((tag("seconds"), space0, char(':'), space0))(input)?;
    let (input, seconds) = parse_number(input)?;
    let (input, ()) = parse_comma(input)?;

    // Parse nanos: number
    let (input, _) = tuple((tag("nanos"), space0, char(':'), space0))(input)?;
    let (input, nanos) = parse_number(input)?;

    let (input, _) = tuple((space0, char('}')))(input)?;

    Ok((input, (seconds, nanos)))
}

// Parse ChangedFile { reference: "...", reference_mtime: FileTime { ... },
// stale: "...", stale_mtime: FileTime { ... } }
fn parse_changed_file(input: &str) -> IResult<&str, String> {
    let (input, _) = tag("ChangedFile")(input)?;
    let (input, _) = tuple((space0, char('{'), space0))(input)?;

    // Skip reference field
    let (input, _) = tuple((tag("reference"), space0, char(':'), space0))(input)?;
    let (input, _) = parse_quoted_string(input)?;
    let (input, ()) = parse_comma(input)?;

    // Skip reference_mtime field
    let (input, _) = tuple((tag("reference_mtime"), space0, char(':'), space0))(input)?;
    let (input, _) = parse_file_time(input)?;
    let (input, ()) = parse_comma(input)?;

    // Extract stale path
    let (input, _) = tuple((tag("stale"), space0, char(':'), space0))(input)?;
    let (input, stale_path) = parse_quoted_string(input)?;
    let (input, ()) = parse_comma(input)?;

    // Skip stale_mtime field
    let (input, _) = tuple((tag("stale_mtime"), space0, char(':'), space0))(input)?;
    let (input, _) = parse_file_time(input)?;

    let (input, _) = tuple((space0, char('}')))(input)?;

    Ok((input, stale_path))
}

// Parse FsStatusOutdated(StaleItem(ChangedFile { ... }))
fn parse_fs_status_outdated_changed_file(input: &str) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("FsStatusOutdated")(input)?;
    let (input, _) = tuple((char('('), tag("StaleItem"), char('(')))(input)?;

    let (input, path) = parse_changed_file(input)?;

    let (input, _) = tuple((char(')'), char(')')))(input)?;

    Ok((input, RebuildReason::FileChanged { path }))
}

// Parse FsStatusOutdated(StaleDepFingerprint { name: "..." })
fn parse_fs_status_outdated_stale_dep(input: &str) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("FsStatusOutdated")(input)?;
    let (input, _) = tuple((
        char('('),
        tag("StaleDepFingerprint"),
        space0,
        char('{'),
        space0,
    ))(input)?;

    let (input, _) = tuple((tag("name"), space0, char(':'), space0))(input)?;
    let (input, name) = parse_quoted_string(input)?;

    let (input, _) = tuple((space0, char('}'), char(')')))(input)?;

    Ok((
        input,
        RebuildReason::UnitDependencyInfoChanged {
            name,
            old_fingerprint: String::new(),
            new_fingerprint: String::new(),
            context: None,
        },
    ))
}

// Parse modern cargo's UnitDependencyInfoChanged { unit: UnitIndex(N) }.
// The unit index is opaque without a `Units` table we don't have access to,
// so name is left empty. The reason still flows through as a non-root
// dependency change so it doesn't pollute cluster roots.
fn parse_unit_dependency_info_changed_unit_index(
    input: &str,
) -> IResult<&str, RebuildReason> {
    let (input, _) = tag("UnitDependencyInfoChanged")(input)?;
    let (input, _) = tuple((space0, char('{'), space0))(input)?;
    let (input, _) = tuple((tag("unit"), space0, char(':'), space0))(input)?;
    let (input, _) = tuple((tag("UnitIndex"), char('(')))(input)?;
    let (input, _) = parse_number(input)?;
    let (input, _) = tuple((char(')'), space0, char('}')))(input)?;

    Ok((
        input,
        RebuildReason::UnitDependencyInfoChanged {
            name: String::new(),
            old_fingerprint: String::new(),
            new_fingerprint: String::new(),
            context: None,
        },
    ))
}

// Main parser for dirty reasons
fn parse_dirty_reason_content(input: &str) -> IResult<&str, RebuildReason> {
    alt((
        parse_env_var_changed,
        parse_unit_dependency_info_changed,
        parse_unit_dependency_info_changed_unit_index,
        parse_target_configuration_changed,
        parse_profile_configuration_changed,
        parse_rustflags_changed,
        parse_features_changed,
        parse_fs_status_outdated_stale_dep,
        parse_fs_status_outdated_changed_file,
        parse_unknown_reason,
    ))(input)
}

// Fallback parser for unknown/unrecognized dirty reasons
#[allow(clippy::unnecessary_wraps, reason = "Needed for nom parser combinator")]
fn parse_unknown_reason(input: &str) -> IResult<&str, RebuildReason> {
    let content = input.trim().to_string();
    Ok(("", RebuildReason::Unknown(content)))
}

// Parse the full "dirty: <reason>" pattern
#[must_use]
pub fn parse_rebuild_reason(input: &str) -> Option<RebuildReason> {
    // Only parse "dirty:" lines - the "stale: changed" lines are redundant
    // with FsStatusOutdated(StaleItem(ChangedFile...)) and report the wrong package
    // context
    input.find("dirty:").and_then(|dirty_start| {
        let dirty_content = &input[dirty_start + 6..].trim_start();

        match parse_dirty_reason_content(dirty_content) {
            Ok((_, reason)) => Some(reason),
            Err(_) => None,
        }
    })
}

/// Parse a complete rebuild entry with package context from a cargo log line
#[must_use]
pub fn parse_rebuild_entry(input: &str) -> Option<ParsedRebuildEntry> {
    let reason = parse_rebuild_reason(input)?;
    let package = extract_package_context(input);
    Some(ParsedRebuildEntry::new(package, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_package_context_from_cargo_log() {
        let log_line = r#"    0.102058909s  INFO prepare_target{force=false package_id=libz-sys v1.1.23 target="build-script-build"}: cargo::core::compiler::fingerprint:     dirty: EnvVarChanged { name: "CC", old_value: Some("gcc"), new_value: None }"#;

        let entry = parse_rebuild_entry(log_line).unwrap();
        assert_eq!(entry.package.package_id, "libz-sys v1.1.23");
        assert_eq!(entry.package.target, Some("build-script-build".to_string()));
    }

    #[test]
    fn handles_missing_package_context() {
        let log_line =
            r#"dirty: EnvVarChanged { name: "CC", old_value: Some("gcc"), new_value: None }"#;

        let entry = parse_rebuild_entry(log_line).unwrap();
        assert_eq!(entry.package.package_id, "unknown");
        assert_eq!(entry.package.target, None);
    }

    #[test]
    fn extracts_package_without_target() {
        let log_line = r"prepare_target{force=false package_id=serde v1.0.0}: dirty: TargetConfigurationChanged";

        let entry = parse_rebuild_entry(log_line).unwrap();
        assert_eq!(entry.package.package_id, "serde v1.0.0");
        assert_eq!(entry.package.target, None);
    }

    #[test]
    fn handles_env_var_changed_with_none_to_some() {
        let log_line = r#"dirty: EnvVarChanged { name: "RUST_LOG", old_value: None, new_value: Some("debug") }"#;
        let result = parse_rebuild_reason(log_line);

        assert_eq!(
            result,
            Some(RebuildReason::EnvVarChanged {
                name: "RUST_LOG".to_string(),
                old_value: None,
                new_value: Some("debug".to_string()),
            })
        );
    }

    #[test]
    fn handles_env_var_changed_with_complex_paths() {
        let log_line = r#"dirty: EnvVarChanged { name: "PATH", old_value: Some("/usr/bin:/bin"), new_value: Some("/nix/store/abc:/usr/bin:/bin") }"#;
        let result = parse_rebuild_reason(log_line);

        assert_eq!(
            result,
            Some(RebuildReason::EnvVarChanged {
                name: "PATH".to_string(),
                old_value: Some("/usr/bin:/bin".to_string()),
                new_value: Some("/nix/store/abc:/usr/bin:/bin".to_string()),
            })
        );
    }

    #[test]
    fn handles_unit_dependency_info_changed() {
        let log_line = r#"dirty: UnitDependencyInfoChanged { old_name: "rusqlite", old_fingerprint: 5920731552898212716, new_name: "rusqlite", new_fingerprint: 7766129310588964256 }"#;
        let result = parse_rebuild_reason(log_line);

        assert_eq!(
            result,
            Some(RebuildReason::UnitDependencyInfoChanged {
                name: "rusqlite".to_string(),
                old_fingerprint: "5920731552898212716".to_string(),
                new_fingerprint: "7766129310588964256".to_string(),
                context: None,
            })
        );
    }

    #[test]
    fn handles_target_configuration_changed() {
        let log_line = r"dirty: TargetConfigurationChanged";
        let result = parse_rebuild_reason(log_line);

        assert_eq!(result, Some(RebuildReason::TargetConfigurationChanged));
    }

    #[test]
    fn handles_fs_status_outdated_with_file_change() {
        let log_line = r#"dirty: FsStatusOutdated(StaleItem(ChangedFile { reference: "/tmp/.tmp6t5LHE/target/debug/.fingerprint/target-test-d08e845c3c592b51/dep-bin-target-test", reference_mtime: FileTime { seconds: 1763310414, nanos: 599971397 }, stale: "/tmp/.tmp6t5LHE/src/main.rs", stale_mtime: FileTime { seconds: 1763310414, nanos: 663971117 } }))"#;
        let result = parse_rebuild_reason(log_line);

        assert_eq!(
            result,
            Some(RebuildReason::FileChanged {
                path: "/tmp/.tmp6t5LHE/src/main.rs".to_string(),
            })
        );
    }

    #[test]
    fn handles_unknown_dirty_reason_format() {
        let log_line = r#"dirty: SomeUnknownReason { data: "value" }"#;
        let result = parse_rebuild_reason(log_line);

        assert_eq!(
            result,
            Some(RebuildReason::Unknown(
                r#"SomeUnknownReason { data: "value" }"#.to_string()
            ))
        );
    }

    #[test]
    fn handles_rustflags_changed() {
        let log_line = r#"dirty: RustflagsChanged { old: ["--cfg", "test"], new: ["--cfg", "test", "-C", "target-cpu=native"] }"#;
        let result = parse_rebuild_reason(log_line);

        assert_eq!(
            result,
            Some(RebuildReason::RustflagsChanged {
                old: vec!["--cfg".to_string(), "test".to_string()],
                new: vec![
                    "--cfg".to_string(),
                    "test".to_string(),
                    "-C".to_string(),
                    "target-cpu=native".to_string()
                ],
            })
        );
    }

    #[test]
    fn handles_rustflags_changed_empty_vecs() {
        let log_line = r#"dirty: RustflagsChanged { old: [], new: ["-C", "opt-level=3"] }"#;
        let result = parse_rebuild_reason(log_line);

        assert_eq!(
            result,
            Some(RebuildReason::RustflagsChanged {
                old: vec![],
                new: vec!["-C".to_string(), "opt-level=3".to_string()],
            })
        );
    }

    #[test]
    fn handles_features_changed() {
        let log_line = r#"dirty: FeaturesChanged { old: "default", new: "default,serde" }"#;
        let result = parse_rebuild_reason(log_line);

        assert_eq!(
            result,
            Some(RebuildReason::FeaturesChanged {
                old: "default".to_string(),
                new: "default,serde".to_string(),
            })
        );
    }

    #[test]
    fn handles_profile_configuration_changed() {
        let log_line = r"dirty: ProfileConfigurationChanged";
        let result = parse_rebuild_reason(log_line);

        assert_eq!(result, Some(RebuildReason::ProfileConfigurationChanged));
    }

    #[test]
    fn returns_none_for_lines_without_dirty_marker() {
        let log_line =
            r"    0.102058909s  INFO cargo::core::compiler::fingerprint: some other log message";
        let result = parse_rebuild_reason(log_line);

        assert_eq!(result, None);
    }

    #[test]
    fn handles_malformed_input_gracefully() {
        let lines_without_dirty = vec![r"", r"some random log line"];

        for line in lines_without_dirty {
            let result = parse_rebuild_reason(line);
            assert_eq!(
                result, None,
                "Expected None for line without dirty marker: {line}"
            );
        }

        let malformed_with_dirty = vec![
            r#"dirty: EnvVarChanged { name: "CC", old_value: Some("gcc")"#,
            r#"dirty: EnvVarChanged { name: CC", old_value: Some("gcc"), new_value: None }"#,
            r#"dirty: UnitDependencyInfoChanged { old_name: "rusqlite""#,
            r"dirty:",
        ];

        for line in malformed_with_dirty {
            let result = parse_rebuild_reason(line);
            assert!(
                matches!(result, Some(RebuildReason::Unknown(_))),
                "Expected Unknown for malformed dirty line: {line}"
            );
        }
    }
}
