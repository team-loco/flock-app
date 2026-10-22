pub fn display_name(input: &str) -> Option<String> {
    let trimmed = input.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed() {
        assert_eq!(display_name("  prod east "), Some("prod east".to_owned()));
    }

    #[test]
    fn blank_input_resets_to_the_default_name() {
        assert_eq!(display_name(""), None);
        assert_eq!(display_name("   "), None);
    }
}
