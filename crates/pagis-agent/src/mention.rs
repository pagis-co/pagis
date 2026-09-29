//! @-mention detection for group-channel triggers. Plain-text
//! matching, Slack-style: `@` followed by the agent's name,
//! case-insensitive, on word boundaries. `user@sage.example` is an
//! address, not a mention.

/// True when `text` mentions the agent named `name`.
pub fn is_mentioned(text: &str, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let name = name.to_lowercase();
    let mut search_from = 0;
    while let Some(at) = text[search_from..].find('@') {
        let at = search_from + at;
        search_from = at + 1;
        // A word character before the `@` makes it an address infix.
        if text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric())
        {
            continue;
        }
        let after_at = &text[at + 1..];
        if !after_at.starts_with(&name) {
            continue;
        }
        // The name must end on a word boundary.
        if after_at[name.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_mentioned;

    #[test]
    fn a_plain_mention_matches() {
        assert!(is_mentioned("hey @Sage, take this", "Sage"));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(is_mentioned("@sage please", "Sage"));
        assert!(is_mentioned("@SAGE please", "sage"));
    }

    #[test]
    fn a_mention_at_the_end_matches() {
        assert!(is_mentioned("over to you @Sage", "Sage"));
    }

    #[test]
    fn text_without_the_name_does_not_match() {
        assert!(!is_mentioned("hey team, status?", "Sage"));
    }

    #[test]
    fn the_bare_name_without_at_does_not_match() {
        assert!(!is_mentioned("Sage should take this", "Sage"));
    }

    #[test]
    fn a_longer_word_does_not_match() {
        assert!(!is_mentioned("@Sagebrush is a plant", "Sage"));
    }

    #[test]
    fn an_email_address_does_not_match() {
        assert!(!is_mentioned("mail user@sage.example", "Sage"));
    }

    #[test]
    fn punctuation_after_the_name_matches() {
        assert!(is_mentioned("@Sage: run the report", "Sage"));
    }

    #[test]
    fn an_empty_name_never_matches() {
        assert!(!is_mentioned("@ everyone", ""));
    }

    #[test]
    fn a_multi_word_name_matches() {
        assert!(is_mentioned("ping @Data Miner now", "Data Miner"));
    }
}
