//! Bounded cumulative text/cursor, never an unbounded queue of synthesis jobs.
#[derive(Default)]
pub(crate) struct SpeechQueue {
    text: String,
    cursor: usize,
    done: bool,
}
impl SpeechQueue {
    pub(crate) fn update(&mut self, text: &str, done: bool) -> Result<(), &'static str> {
        if text.len() > 64_000 {
            return Err("The spoken reply exceeded its size limit");
        }
        if done && text == self.text.trim_end() {
            self.done = true;
            return Ok(());
        }
        if !text.starts_with(&self.text) {
            return Err("The reply changed while being spoken. Please ask again.");
        }
        self.text = text.into();
        self.done |= done;
        Ok(())
    }
    pub(crate) fn finished(&self) -> bool {
        self.done && self.cursor >= self.text.len()
    }
    pub(crate) fn next(&mut self) -> Option<String> {
        let text = &self.text[self.cursor..];
        if text.trim().is_empty() {
            if self.done {
                self.cursor = self.text.len();
            }
            return None;
        }
        let mut end = None;
        for (i, c) in text.char_indices() {
            let after = i + c.len_utf8();
            let sentence = matches!(c, '.' | '?' | '!' | '。' | '？' | '！');
            let boundary = text[after..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
                || self.done && after == text.len()
                || matches!(c, '。' | '？' | '！');
            if sentence && boundary {
                let word = text[..i].split_whitespace().last().unwrap_or("");
                let list_number = word.chars().all(|c| c.is_ascii_digit())
                    && !word.is_empty()
                    && text[..i].trim() == word
                    && !text[after..].trim().is_empty();
                if c == '.'
                    && (list_number
                        || matches!(word, "Dr" | "Mr" | "Mrs" | "Ms" | "Prof" | "e.g" | "i.e")
                        || word.len() == 1 && word.chars().all(char::is_alphabetic))
                {
                    continue;
                }
                end = Some(after);
                break;
            }
            if after >= 600 {
                break;
            }
        }
        if end.is_none() && text.len() > 600 {
            let boundary = text
                .char_indices()
                .take_while(|(i, _)| *i <= 600)
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            end = Some(
                text[..boundary]
                    .rfind(char::is_whitespace)
                    .filter(|i| *i > 0)
                    .unwrap_or(boundary),
            );
        }
        if end.is_none() && self.done {
            end = Some(text.len());
        }
        let end = end?;
        self.cursor += end;
        Some(text[..end].trim().to_owned())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_sentence_is_ready_before_the_answer_finishes_and_never_repeats() {
        let mut q = SpeechQueue::default();
        q.update("First sentence. Next", false).unwrap();
        assert_eq!(q.next().as_deref(), Some("First sentence."));
        assert_eq!(q.next(), None);
        assert!(!q.finished());
        q.update("First sentence. Next sentence.", true).unwrap();
        assert_eq!(q.next().as_deref(), Some("Next sentence."));
        assert!(q.finished());
        assert_eq!(q.next(), None);
    }
    #[test]
    fn fragments_abbreviations_decimals_and_unicode_are_safe() {
        let mut q = SpeechQueue::default();
        q.update("Dr. Patel used 3.14. It", false).unwrap();
        assert_eq!(q.next().as_deref(), Some("Dr. Patel used 3.14."));
        let mut q = SpeechQueue::default();
        q.update(&"字".repeat(600), false).unwrap();
        let first = q.next().unwrap();
        assert!(first.len() <= 600 && first.len() % 3 == 0);
        let mut q = SpeechQueue::default();
        q.update("12.", true).unwrap();
        assert_eq!(q.next().as_deref(), Some("12."));
        assert!(q.finished());
    }
    #[test]
    fn numbered_lists_do_not_synthesize_the_number_as_a_separate_sentence() {
        let mut q = SpeechQueue::default();
        q.update("1. First explanation. 2. Next explanation.", true)
            .unwrap();
        assert_eq!(q.next().as_deref(), Some("1. First explanation."));
        assert_eq!(q.next().as_deref(), Some("2. Next explanation."));
    }
    #[test]
    fn rewritten_or_oversized_answers_are_rejected() {
        let mut q = SpeechQueue::default();
        q.update("Stable. ", false).unwrap();
        assert!(q.update("Changed.", true).is_err());
        assert!(q.update(&"x".repeat(64001), true).is_err());
    }
}
