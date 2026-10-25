#[must_use]
pub fn compact_name_list(crate_names: &[String], shown_name_limit: usize) -> String {
    let shown_names =
        shorten_shared_prefixes(&crate_names[..crate_names.len().min(shown_name_limit)]);
    let hidden_name_count = crate_names.len().saturating_sub(shown_name_limit);
    if hidden_name_count > 0 {
        format!("{shown_names}, +{hidden_name_count} more")
    } else {
        shown_names
    }
}

#[must_use]
pub fn shorten_shared_prefixes(crate_names: &[String]) -> String {
    crate_names
        .iter()
        .fold(
            ShortenedNames {
                group_prefix: String::new(),
                shortened_names: Vec::new(),
            },
            |mut shortened_names, crate_name| {
                match crate_name.strip_prefix(&shortened_names.group_prefix) {
                    Some(name_rest) if !shortened_names.group_prefix.is_empty() => {
                        shortened_names
                            .shortened_names
                            .push(format!("-{name_rest}"));
                    }
                    _ => {
                        shortened_names.group_prefix = crate_name
                            .split_once('-')
                            .map(|(name_prefix, _)| format!("{name_prefix}-"))
                            .unwrap_or_default();
                        shortened_names.shortened_names.push(crate_name.clone());
                    }
                }
                shortened_names
            },
        )
        .shortened_names
        .join(", ")
}

struct ShortenedNames {
    group_prefix: String,
    shortened_names: Vec<String>,
}
