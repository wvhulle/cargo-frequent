use nom::{
    IResult,
    branch::alt,
    bytes::complete::{tag, take_until},
    character::complete::{char, digit1, space0},
    combinator::{map, rest},
    multi::separated_list0,
    sequence::{delimited, preceded, terminated, tuple},
};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum DirtyReason {
    EnvVarSet {
        variable_name: String,
        current_value: String,
    },
    EnvVarUnset {
        variable_name: String,
        previous_value: String,
    },
    EnvVarChanged {
        variable_name: String,
        previous_value: String,
        current_value: String,
    },
    DependencyRebuilt,
    RustflagsChanged {
        previous_flags: Vec<String>,
        current_flags: Vec<String>,
    },
    FeaturesChanged {
        previous_features: String,
        current_features: String,
    },
    ProfileChanged,
    TargetConfigChanged,
    FileChanged {
        file_path: String,
    },
    Unrecognized {
        reason_text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirtyUnitEntry {
    pub package_name: String,
    pub is_build_script: bool,
    pub dirty_reason: DirtyReason,
}

const BUILD_SCRIPT_TARGET: &str = "build-script-build";

pub fn parse_fingerprint_log_line(log_line: &str) -> Option<DirtyUnitEntry> {
    let (dirty_payload, ()) = fingerprint_dirty_prefix(log_line).ok()?;
    let (_, dirty_reason) = dirty_reason(dirty_payload).ok()?;
    let package_name = log_line
        .split_once("package_id=")
        .and_then(|(_, package_text)| package_text.split_whitespace().next())
        .unwrap_or("unknown")
        .to_string();
    Some(DirtyUnitEntry {
        package_name,
        is_build_script: log_line.contains(&format!("target=\"{BUILD_SCRIPT_TARGET}\"")),
        dirty_reason,
    })
}

fn fingerprint_dirty_prefix(input: &str) -> IResult<&str, ()> {
    let (input, _) = take_until("fingerprint:")(input)?;
    let (input, _) = tag("fingerprint:")(input)?;
    let (input, _) = take_until("dirty:")(input)?;
    let (input, _) = tag("dirty:")(input)?;
    let (input, _) = space0(input)?;
    Ok((input, ()))
}

fn dirty_reason(input: &str) -> IResult<&str, DirtyReason> {
    alt((
        env_var_changed,
        dependency_info_changed,
        map(tag("TargetConfigurationChanged"), |_| {
            DirtyReason::TargetConfigChanged
        }),
        map(tag("ProfileConfigurationChanged"), |_| {
            DirtyReason::ProfileChanged
        }),
        rustflags_changed,
        features_changed,
        stale_dependency_fingerprint,
        changed_file,
        map(rest, |reason_text: &str| DirtyReason::Unrecognized {
            reason_text: reason_text.trim().to_string(),
        }),
    ))(input)
}

fn quoted_string(input: &str) -> IResult<&str, String> {
    delimited(char('"'), map(take_until("\""), str::to_string), char('"'))(input)
}

fn field_name<'input>(name: &'static str) -> impl FnMut(&'input str) -> IResult<&'input str, ()> {
    map(
        tuple((space0, tag(name), space0, char(':'), space0)),
        |_| (),
    )
}

fn field_separator(input: &str) -> IResult<&str, ()> {
    map(tuple((space0, char(','), space0)), |_| ())(input)
}

fn struct_open<'input>(name: &'static str) -> impl FnMut(&'input str) -> IResult<&'input str, ()> {
    map(tuple((tag(name), space0, char('{'))), |_| ())
}

fn struct_close(input: &str) -> IResult<&str, ()> {
    map(tuple((space0, char('}'))), |_| ())(input)
}

fn optional_quoted_string(input: &str) -> IResult<&str, Option<String>> {
    alt((
        map(tag("None"), |_| None),
        map(
            preceded(tag("Some("), terminated(quoted_string, char(')'))),
            Some,
        ),
    ))(input)
}

fn env_var_changed(input: &str) -> IResult<&str, DirtyReason> {
    let (input, ()) = struct_open("EnvVarChanged")(input)?;
    let (input, ()) = field_name("name")(input)?;
    let (input, variable_name) = quoted_string(input)?;
    let (input, ()) = field_separator(input)?;
    let (input, ()) = field_name("old_value")(input)?;
    let (input, previous_value) = optional_quoted_string(input)?;
    let (input, ()) = field_separator(input)?;
    let (input, ()) = field_name("new_value")(input)?;
    let (input, current_value) = optional_quoted_string(input)?;
    let (input, ()) = struct_close(input)?;
    let parsed_reason = match (previous_value, current_value) {
        (Some(previous_value), Some(current_value)) => DirtyReason::EnvVarChanged {
            variable_name,
            previous_value,
            current_value,
        },
        (Some(previous_value), None) => DirtyReason::EnvVarUnset {
            variable_name,
            previous_value,
        },
        (None, Some(current_value)) => DirtyReason::EnvVarSet {
            variable_name,
            current_value,
        },
        (None, None) => DirtyReason::Unrecognized {
            reason_text: format!("EnvVarChanged {{ name: {variable_name:?} }}"),
        },
    };
    Ok((input, parsed_reason))
}

fn dependency_info_changed(input: &str) -> IResult<&str, DirtyReason> {
    let (input, ()) = struct_open("UnitDependencyInfoChanged")(input)?;
    let (input, ()) = alt((
        map(
            tuple((
                field_name("old_name"),
                quoted_string,
                field_separator,
                field_name("old_fingerprint"),
                digit1,
                field_separator,
                field_name("new_name"),
                quoted_string,
                field_separator,
                field_name("new_fingerprint"),
                digit1,
            )),
            |_| (),
        ),
        map(
            tuple((field_name("unit"), tag("UnitIndex("), digit1, char(')'))),
            |_| (),
        ),
    ))(input)?;
    let (input, ()) = struct_close(input)?;
    Ok((input, DirtyReason::DependencyRebuilt))
}

fn stale_dependency_fingerprint(input: &str) -> IResult<&str, DirtyReason> {
    let (input, _) = tuple((
        tag("FsStatusOutdated(StaleDepFingerprint"),
        space0,
        char('{'),
        field_name("name"),
        quoted_string,
        struct_close,
        char(')'),
    ))(input)?;
    Ok((input, DirtyReason::DependencyRebuilt))
}

fn string_list(input: &str) -> IResult<&str, Vec<String>> {
    delimited(
        tuple((char('['), space0)),
        separated_list0(field_separator, quoted_string),
        tuple((space0, char(']'))),
    )(input)
}

fn rustflags_changed(input: &str) -> IResult<&str, DirtyReason> {
    let (input, ()) = struct_open("RustflagsChanged")(input)?;
    let (input, ()) = field_name("old")(input)?;
    let (input, previous_flags) = string_list(input)?;
    let (input, ()) = field_separator(input)?;
    let (input, ()) = field_name("new")(input)?;
    let (input, current_flags) = string_list(input)?;
    let (input, ()) = struct_close(input)?;
    Ok((
        input,
        DirtyReason::RustflagsChanged {
            previous_flags,
            current_flags,
        },
    ))
}

fn features_changed(input: &str) -> IResult<&str, DirtyReason> {
    let (input, ()) = struct_open("FeaturesChanged")(input)?;
    let (input, ()) = field_name("old")(input)?;
    let (input, previous_features) = quoted_string(input)?;
    let (input, ()) = field_separator(input)?;
    let (input, ()) = field_name("new")(input)?;
    let (input, current_features) = quoted_string(input)?;
    let (input, ()) = struct_close(input)?;
    Ok((
        input,
        DirtyReason::FeaturesChanged {
            previous_features,
            current_features,
        },
    ))
}

fn file_time(input: &str) -> IResult<&str, ()> {
    map(
        tuple((
            struct_open("FileTime"),
            field_name("seconds"),
            digit1,
            field_separator,
            field_name("nanos"),
            digit1,
            struct_close,
        )),
        |_| (),
    )(input)
}

fn changed_file(input: &str) -> IResult<&str, DirtyReason> {
    let (input, _) = tag("FsStatusOutdated(StaleItem(")(input)?;
    let (input, ()) = struct_open("ChangedFile")(input)?;
    let (input, ()) = field_name("reference")(input)?;
    let (input, _) = quoted_string(input)?;
    let (input, ()) = field_separator(input)?;
    let (input, ()) = field_name("reference_mtime")(input)?;
    let (input, ()) = file_time(input)?;
    let (input, ()) = field_separator(input)?;
    let (input, ()) = field_name("stale")(input)?;
    let (input, file_path) = quoted_string(input)?;
    let (input, ()) = field_separator(input)?;
    let (input, ()) = field_name("stale_mtime")(input)?;
    let (input, ()) = file_time(input)?;
    let (input, ()) = struct_close(input)?;
    let (input, _) = tag("))")(input)?;
    Ok((input, DirtyReason::FileChanged { file_path }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason_of(dirty_payload: &str) -> DirtyReason {
        let log_line = format!(
            r#"   0.1s  INFO prepare_target{{force=false package_id=libz-sys v1.1.23 target="build-script-build"}}: cargo::compiler::fingerprint:     dirty: {dirty_payload}"#
        );
        parse_fingerprint_log_line(&log_line)
            .expect("fingerprint dirty line")
            .dirty_reason
    }

    #[test]
    fn extracts_package_and_build_script_target() {
        let log_line = r#"    0.102058909s  INFO prepare_target{force=false package_id=libz-sys v1.1.23 target="build-script-build"}: cargo::core::compiler::fingerprint:     dirty: TargetConfigurationChanged"#;
        let dirty_entry = parse_fingerprint_log_line(log_line).unwrap();
        assert_eq!(dirty_entry.package_name, "libz-sys");
        assert!(dirty_entry.is_build_script, "build script target");
    }

    #[test]
    fn library_target_is_not_build_script() {
        let log_line = r#"INFO prepare_target{force=false package_id=probe v0.1.0 (/tmp/probe) target="probe"}: cargo::compiler::fingerprint:     dirty: ProfileConfigurationChanged"#;
        let dirty_entry = parse_fingerprint_log_line(log_line).unwrap();
        assert_eq!(dirty_entry.package_name, "probe");
        assert!(!dirty_entry.is_build_script, "library target");
    }

    #[test]
    fn splits_env_var_changes_by_presence() {
        assert_eq!(
            reason_of(
                r#"EnvVarChanged { name: "RUST_LOG", old_value: None, new_value: Some("debug") }"#
            ),
            DirtyReason::EnvVarSet {
                variable_name: "RUST_LOG".to_string(),
                current_value: "debug".to_string()
            }
        );
        assert_eq!(
            reason_of(r#"EnvVarChanged { name: "CC", old_value: Some("gcc"), new_value: None }"#),
            DirtyReason::EnvVarUnset {
                variable_name: "CC".to_string(),
                previous_value: "gcc".to_string()
            }
        );
        assert_eq!(
            reason_of(
                r#"EnvVarChanged { name: "PATH", old_value: Some("/usr/bin:/bin"), new_value: Some("/nix/store/abc:/usr/bin:/bin") }"#
            ),
            DirtyReason::EnvVarChanged {
                variable_name: "PATH".to_string(),
                previous_value: "/usr/bin:/bin".to_string(),
                current_value: "/nix/store/abc:/usr/bin:/bin".to_string()
            }
        );
    }

    #[test]
    fn recognizes_dependency_rebuilds_in_all_formats() {
        [
            r#"UnitDependencyInfoChanged { old_name: "rusqlite", old_fingerprint: 5920731552898212716, new_name: "rusqlite", new_fingerprint: 7766129310588964256 }"#,
            "UnitDependencyInfoChanged { unit: UnitIndex(12) }",
            r#"FsStatusOutdated(StaleDepFingerprint { name: "numpy" })"#,
        ]
        .into_iter()
        .for_each(|dirty_payload| {
            assert_eq!(reason_of(dirty_payload), DirtyReason::DependencyRebuilt, "{dirty_payload}");
        });
    }

    #[test]
    fn extracts_changed_file_path() {
        assert_eq!(
            reason_of(
                r#"FsStatusOutdated(StaleItem(ChangedFile { reference: "/tmp/x/target/debug/.fingerprint/t-d08e845c3c592b51/dep-bin-t", reference_mtime: FileTime { seconds: 1763310414, nanos: 599971397 }, stale: "/tmp/x/src/main.rs", stale_mtime: FileTime { seconds: 1763310414, nanos: 663971117 } }))"#
            ),
            DirtyReason::FileChanged {
                file_path: "/tmp/x/src/main.rs".to_string()
            }
        );
    }

    #[test]
    fn parses_flag_and_feature_changes() {
        assert_eq!(
            reason_of(r#"RustflagsChanged { old: [], new: ["-C", "opt-level=3"] }"#),
            DirtyReason::RustflagsChanged {
                previous_flags: vec![],
                current_flags: vec!["-C".to_string(), "opt-level=3".to_string()]
            }
        );
        assert_eq!(
            reason_of(r#"FeaturesChanged { old: "default", new: "default,serde" }"#),
            DirtyReason::FeaturesChanged {
                previous_features: "default".to_string(),
                current_features: "default,serde".to_string()
            }
        );
        assert_eq!(
            reason_of("ProfileConfigurationChanged"),
            DirtyReason::ProfileChanged
        );
    }

    #[test]
    fn keeps_unknown_and_malformed_reasons_as_text() {
        [
            r#"SomeUnknownReason { data: "value" }"#,
            r#"EnvVarChanged { name: "CC", old_value: Some("gcc")"#,
            r#"UnitDependencyInfoChanged { old_name: "rusqlite""#,
        ]
        .into_iter()
        .for_each(|dirty_payload| {
            assert_eq!(
                reason_of(dirty_payload),
                DirtyReason::Unrecognized {
                    reason_text: dirty_payload.to_string()
                }
            );
        });
    }

    #[test]
    fn ignores_lines_outside_fingerprint_dirty_log() {
        [
            "",
            "some random log line",
            "    0.1s  INFO cargo::compiler::fingerprint: some other log message",
            "dirty: ProfileConfigurationChanged",
        ]
        .into_iter()
        .for_each(|log_line| {
            assert_eq!(parse_fingerprint_log_line(log_line), None, "{log_line}");
        });
    }
}
