use std::fmt::{Display, Formatter, Result as FmtResult};

use serde::Serialize;

const SHOWN_TOKEN_LIMIT: usize = 4;
const STORE_HASH_LENGTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum TokenSeparator {
    Whitespace,
    Colon,
    WholeValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct EnvValueTokenDiff {
    pub token_separator: TokenSeparator,
    pub previous_token_count: Option<usize>,
    pub current_token_count: Option<usize>,
    pub removed_tokens: Vec<String>,
    pub added_tokens: Vec<String>,
    pub reordered_only: bool,
}

impl EnvValueTokenDiff {
    #[must_use]
    pub fn between(previous_value: Option<&str>, current_value: Option<&str>) -> Option<Self> {
        if previous_value == current_value {
            return None;
        }
        let token_separator = choose_token_separator(previous_value, current_value);
        let previous_tokens = previous_value.map(|value| split_tokens(value, token_separator));
        let current_tokens = current_value.map(|value| split_tokens(value, token_separator));
        let previous_token_list = previous_tokens.clone().unwrap_or_default();
        let current_token_list = current_tokens.clone().unwrap_or_default();

        let removed_tokens: Vec<String> = previous_token_list
            .iter()
            .filter(|token| !current_token_list.contains(token))
            .cloned()
            .collect();
        let added_tokens: Vec<String> = current_token_list
            .iter()
            .filter(|token| !previous_token_list.contains(token))
            .cloned()
            .collect();
        let reordered_only = previous_value.is_some()
            && current_value.is_some()
            && removed_tokens.is_empty()
            && added_tokens.is_empty();

        Some(Self {
            token_separator,
            previous_token_count: previous_tokens.as_ref().map(Vec::len),
            current_token_count: current_tokens.as_ref().map(Vec::len),
            removed_tokens,
            added_tokens,
            reordered_only,
        })
    }

    pub fn changed_tokens(&self) -> impl Iterator<Item = &str> {
        self.removed_tokens
            .iter()
            .chain(&self.added_tokens)
            .map(String::as_str)
    }

    #[must_use]
    pub const fn became_unset(&self) -> bool {
        self.previous_token_count.is_some() && self.current_token_count.is_none()
    }

    #[must_use]
    pub const fn became_set(&self) -> bool {
        self.previous_token_count.is_none() && self.current_token_count.is_some()
    }
}

fn choose_token_separator(
    previous_value: Option<&str>,
    current_value: Option<&str>,
) -> TokenSeparator {
    let both_values = [previous_value, current_value];
    let present_values = both_values.iter().flatten();
    if present_values
        .clone()
        .any(|value| value.contains(char::is_whitespace))
    {
        TokenSeparator::Whitespace
    } else if present_values.clone().any(|value| value.contains(':')) {
        TokenSeparator::Colon
    } else {
        TokenSeparator::WholeValue
    }
}

fn split_tokens(value: &str, token_separator: TokenSeparator) -> Vec<String> {
    match token_separator {
        TokenSeparator::Whitespace => value.split_whitespace().map(str::to_string).collect(),
        TokenSeparator::Colon => value
            .split(':')
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect(),
        TokenSeparator::WholeValue => vec![value.to_string()],
    }
}

fn abbreviate_nix_store_hashes(token: &str) -> String {
    let store_prefix = "/nix/store/";
    let mut abbreviated_token = String::with_capacity(token.len());
    let mut remaining_text = token;
    while let Some(prefix_position) = remaining_text.find(store_prefix) {
        let after_prefix = &remaining_text[prefix_position + store_prefix.len()..];
        abbreviated_token.push_str(&remaining_text[..prefix_position + store_prefix.len()]);
        let hash_length = after_prefix
            .find('-')
            .filter(|dash_position| *dash_position == STORE_HASH_LENGTH);
        if let Some(hash_length) = hash_length {
            abbreviated_token.push('…');
            remaining_text = &after_prefix[hash_length..];
        } else {
            remaining_text = after_prefix;
        }
    }
    abbreviated_token.push_str(remaining_text);
    abbreviated_token
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

fn describe_token_count(token_count: usize) -> String {
    if token_count == 1 {
        "1 token".to_string()
    } else {
        format!("{token_count} tokens")
    }
}

impl Display for EnvValueTokenDiff {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match (self.previous_token_count, self.current_token_count) {
            (None, Some(_)) => write!(f, "unset -> {}", format_token_list(&self.added_tokens)),
            (Some(_), None) => write!(f, "{} -> unset", format_token_list(&self.removed_tokens)),
            (Some(previous_count), Some(_)) if self.reordered_only => write!(
                f,
                "same {} in a different order",
                describe_token_count(previous_count)
            ),
            _ => match (self.removed_tokens.as_slice(), self.added_tokens.as_slice()) {
                ([], added) => write!(f, "added {}", format_token_list(added)),
                (removed, []) => write!(f, "removed {}", format_token_list(removed)),
                (removed, added) => write!(
                    f,
                    "{} -> {}",
                    format_token_list(removed),
                    format_token_list(added)
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolates_single_changed_flag() {
        let token_diff = EnvValueTokenDiff::between(
            Some("-B/nix/gcc -isystem /inc -frandom-seed=p11zjgl8gg"),
            Some("-B/nix/gcc -isystem /inc -frandom-seed=f020dci6a9"),
        )
        .expect("values differ");
        assert_eq!(token_diff.token_separator, TokenSeparator::Whitespace);
        assert_eq!(token_diff.removed_tokens, vec!["-frandom-seed=p11zjgl8gg"]);
        assert_eq!(token_diff.added_tokens, vec!["-frandom-seed=f020dci6a9"]);
        assert_eq!(
            token_diff.to_string(),
            "-frandom-seed=p11zjgl8gg -> -frandom-seed=f020dci6a9"
        );
    }

    #[test]
    fn detects_reordered_search_path() {
        let token_diff =
            EnvValueTokenDiff::between(Some("/a:/b:/c"), Some("/c:/a:/b")).expect("values differ");
        assert_eq!(token_diff.token_separator, TokenSeparator::Colon);
        assert!(token_diff.reordered_only, "same entries");
        assert_eq!(token_diff.to_string(), "same 3 tokens in a different order");
    }

    #[test]
    fn equal_values_have_no_diff() {
        assert_eq!(EnvValueTokenDiff::between(Some("x"), Some("x")), None);
        assert_eq!(EnvValueTokenDiff::between(None, None), None);
    }

    #[test]
    fn describes_unset_transitions() {
        let token_diff = EnvValueTokenDiff::between(Some("gcc"), None).expect("values differ");
        assert!(token_diff.became_unset(), "value removed");
        assert_eq!(token_diff.to_string(), "gcc -> unset");
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

    #[test]
    fn truncates_long_token_lists() {
        let token_diff =
            EnvValueTokenDiff::between(None, Some("a b c d e f")).expect("values differ");
        assert_eq!(token_diff.to_string(), "unset -> a b c d (+2 more)");
    }
}
