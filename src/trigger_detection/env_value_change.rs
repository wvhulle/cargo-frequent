use std::fmt::{Display, Formatter, Result as FmtResult};

use serde::Serialize;

use crate::trigger_detection::TriggerChange;

const SHOWN_TOKEN_LIMIT: usize = 4;
const INLINE_CHANGE_WIDTH: usize = 72;
const NIX_STORE_PREFIX: &str = "/nix/store/";
const NIX_STORE_HASH_LENGTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum EnvValueChange {
    Set {
        added_tokens: Vec<String>,
    },
    Unset {
        removed_tokens: Vec<String>,
    },
    Reordered {
        token_count: usize,
    },
    TokensReplaced {
        removed_tokens: Vec<String>,
        added_tokens: Vec<String>,
    },
}

impl EnvValueChange {
    #[must_use]
    pub fn set_to(current_value: &str) -> Self {
        Self::Set {
            added_tokens: split_env_tokens(current_value, &[current_value]),
        }
    }

    #[must_use]
    pub fn unset_from(previous_value: &str) -> Self {
        Self::Unset {
            removed_tokens: split_env_tokens(previous_value, &[previous_value]),
        }
    }

    #[must_use]
    pub fn between(previous_value: &str, current_value: &str) -> Option<Self> {
        if previous_value == current_value {
            return None;
        }
        let both_values = [previous_value, current_value];
        let previous_tokens = split_env_tokens(previous_value, &both_values);
        let current_tokens = split_env_tokens(current_value, &both_values);
        let removed_tokens: Vec<String> = previous_tokens
            .iter()
            .filter(|token| !current_tokens.contains(token))
            .cloned()
            .collect();
        let added_tokens: Vec<String> = current_tokens
            .iter()
            .filter(|token| !previous_tokens.contains(token))
            .cloned()
            .collect();
        if removed_tokens.is_empty() && added_tokens.is_empty() {
            return Some(Self::Reordered {
                token_count: previous_tokens.len(),
            });
        }
        Some(Self::TokensReplaced {
            removed_tokens,
            added_tokens,
        })
    }

    #[must_use]
    pub fn to_trigger_change(&self) -> TriggerChange {
        let inline_text = self.to_string();
        match self {
            Self::TokensReplaced {
                removed_tokens,
                added_tokens,
            } if !removed_tokens.is_empty()
                && !added_tokens.is_empty()
                && inline_text.chars().count() > INLINE_CHANGE_WIDTH =>
            {
                TriggerChange::Replacement {
                    removed_text: format_token_list(removed_tokens),
                    added_text: format_token_list(added_tokens),
                }
            }
            _ => TriggerChange::Summary {
                change_text: inline_text,
            },
        }
    }

    #[must_use]
    pub fn changed_tokens(&self) -> Vec<&String> {
        match self {
            Self::Set { added_tokens } => added_tokens.iter().collect(),
            Self::Unset { removed_tokens } => removed_tokens.iter().collect(),
            Self::Reordered { .. } => Vec::new(),
            Self::TokensReplaced {
                removed_tokens,
                added_tokens,
            } => removed_tokens.iter().chain(added_tokens).collect(),
        }
    }
}

fn split_env_tokens(value: &str, compared_values: &[&str]) -> Vec<String> {
    if compared_values
        .iter()
        .any(|compared_value| compared_value.contains(char::is_whitespace))
    {
        return value.split_whitespace().map(str::to_string).collect();
    }
    if compared_values
        .iter()
        .any(|compared_value| compared_value.contains(':'))
    {
        return value
            .split(':')
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect();
    }
    vec![value.to_string()]
}

fn abbreviate_nix_store_hashes(token: &str) -> String {
    token
        .split(NIX_STORE_PREFIX)
        .enumerate()
        .map(|(segment_index, segment)| {
            let hash_ends_at_dash = segment
                .get(NIX_STORE_HASH_LENGTH..)
                .is_some_and(|after_hash| after_hash.starts_with('-'));
            if segment_index > 0 && hash_ends_at_dash {
                format!("{NIX_STORE_PREFIX}…{}", &segment[NIX_STORE_HASH_LENGTH..])
            } else if segment_index > 0 {
                format!("{NIX_STORE_PREFIX}{segment}")
            } else {
                segment.to_string()
            }
        })
        .collect()
}

fn format_token_list(tokens: &[String]) -> String {
    let shown_tokens = tokens
        .iter()
        .take(SHOWN_TOKEN_LIMIT)
        .map(|token| abbreviate_nix_store_hashes(token))
        .collect::<Vec<_>>()
        .join(" ");
    if tokens.len() > SHOWN_TOKEN_LIMIT {
        format!(
            "{shown_tokens} (+{} more)",
            tokens.len() - SHOWN_TOKEN_LIMIT
        )
    } else {
        shown_tokens
    }
}

impl Display for EnvValueChange {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Set { added_tokens } => {
                write!(formatter, "unset -> {}", format_token_list(added_tokens))
            }
            Self::Unset { removed_tokens } => {
                write!(formatter, "{} -> unset", format_token_list(removed_tokens))
            }
            Self::Reordered { token_count } => {
                write!(formatter, "same {token_count} entries in a different order")
            }
            Self::TokensReplaced {
                removed_tokens,
                added_tokens,
            } if added_tokens.is_empty() => {
                write!(formatter, "removed {}", format_token_list(removed_tokens))
            }
            Self::TokensReplaced {
                removed_tokens,
                added_tokens,
            } if removed_tokens.is_empty() => {
                write!(formatter, "added {}", format_token_list(added_tokens))
            }
            Self::TokensReplaced {
                removed_tokens,
                added_tokens,
            } => write!(
                formatter,
                "{} -> {}",
                format_token_list(removed_tokens),
                format_token_list(added_tokens)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolates_single_changed_flag() {
        let env_change = EnvValueChange::between(
            "-B/nix/gcc -isystem /inc -frandom-seed=p11zjgl8gg",
            "-B/nix/gcc -isystem /inc -frandom-seed=f020dci6a9",
        )
        .expect("values differ");
        assert_eq!(
            env_change.to_string(),
            "-frandom-seed=p11zjgl8gg -> -frandom-seed=f020dci6a9"
        );
    }

    #[test]
    fn detects_reordered_search_path() {
        let env_change = EnvValueChange::between("/a:/b:/c", "/c:/a:/b").expect("values differ");
        assert_eq!(env_change, EnvValueChange::Reordered { token_count: 3 });
        assert_eq!(
            env_change.to_string(),
            "same 3 entries in a different order"
        );
    }

    #[test]
    fn equal_values_have_no_change() {
        assert_eq!(EnvValueChange::between("x", "x"), None);
    }

    #[test]
    fn describes_set_and_unset() {
        assert_eq!(
            EnvValueChange::unset_from("gcc").to_string(),
            "gcc -> unset"
        );
        assert_eq!(
            EnvValueChange::set_to("a b c d e f").to_string(),
            "unset -> a b c d (+2 more)"
        );
    }

    #[test]
    fn splits_long_replacements_into_removed_and_added() {
        let env_change = EnvValueChange::between(
            "/nix/store/dnyi059nwg0fyv3iz2npr578p20mwl0d-wayland-1.25.0-dev/lib/pkgconfig:/nix/\
             store/8xfv740cirnhdi4ywby3rnwv4yn7p5dx-libxkbcommon-1.13.2-dev/lib/pkgconfig",
            "/nix/store/6bvizw9qcnw5qz273y2qyix2wnb3r6ns-wayland-1.26.0-dev/lib/pkgconfig:/nix/\
             store/47nlx1dpb8zd73vjp2zms7r8k9xkzndp-libxkbcommon-1.13.2-dev/lib/pkgconfig",
        )
        .expect("values differ");
        assert_eq!(
            env_change.to_trigger_change(),
            TriggerChange::Replacement {
                removed_text: "/nix/store/…-wayland-1.25.0-dev/lib/pkgconfig \
                               /nix/store/…-libxkbcommon-1.13.2-dev/lib/pkgconfig"
                    .to_string(),
                added_text: "/nix/store/…-wayland-1.26.0-dev/lib/pkgconfig \
                             /nix/store/…-libxkbcommon-1.13.2-dev/lib/pkgconfig"
                    .to_string(),
            }
        );
    }

    #[test]
    fn keeps_short_replacements_inline() {
        let env_change = EnvValueChange::between("-frandom-seed=aaa", "-frandom-seed=bbb")
            .expect("values differ");
        assert_eq!(
            env_change.to_trigger_change(),
            TriggerChange::Summary {
                change_text: "-frandom-seed=aaa -> -frandom-seed=bbb".to_string()
            }
        );
    }

    #[test]
    fn abbreviates_store_hashes() {
        assert_eq!(
            abbreviate_nix_store_hashes(
                "-isystem/nix/store/dnyi059nwg0fyv3iz2npr578p20mwl0d-wayland-1.25.0-dev/include"
            ),
            "-isystem/nix/store/…-wayland-1.25.0-dev/include"
        );
        assert_eq!(
            abbreviate_nix_store_hashes("/nix/store/short-x"),
            "/nix/store/short-x"
        );
    }
}
