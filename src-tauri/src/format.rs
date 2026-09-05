// A dictated prompt is always a single line: line breaks from Whisper are
// artifacts, and targets treat them as Enter (terminals run the line, chat
// inputs submit early).
pub fn format_transcript(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_prompt_whitespace() {
        assert_eq!(
            format_transcript("  Refactor   this component   \n\n and add tests.  "),
            "Refactor this component and add tests."
        );
    }

    #[test]
    fn keeps_spoken_prompt_words_intact() {
        assert_eq!(
            format_transcript("use effect open paren"),
            "use effect open paren"
        );
    }
}
