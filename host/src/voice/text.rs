//! Text helpers for the spoken reply: sentence splitting for early TTS, language guess, speech cleanup.

/// Splits streamed text into sentences as soon as each one is complete.
#[derive(Default)]
pub struct SentenceSplitter {
    buf: String,
}

const MIN_SENTENCE_CHARS: usize = 12;

impl SentenceSplitter {
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.buf.push_str(delta);
        let mut out = Vec::new();
        while let Some(end) = self.boundary() {
            let sentence = self.buf[..end].trim().to_string();
            self.buf.drain(..end);
            if !sentence.is_empty() {
                out.push(sentence);
            }
        }
        out
    }

    /// The rest, when the reply has ended.
    pub fn finish(&mut self) -> Option<String> {
        let rest = self.buf.trim().to_string();
        self.buf.clear();
        (!rest.is_empty()).then_some(rest)
    }

    /// Byte index just after a sentence end that is followed by whitespace. Very short fragments wait for
    /// more text so TTS is not started for "Ok." alone, unless a newline ends them.
    fn boundary(&self) -> Option<usize> {
        let mut chars = self.buf.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let terminal = matches!(c, '.' | '!' | '?' | '…' | '。' | ';' | ':');
            let next = chars.peek().map(|(_, n)| *n);
            if c == '\n' && self.buf[..i].trim().chars().count() > 0 {
                return Some(i + 1);
            }
            if terminal && next.is_some_and(char::is_whitespace) {
                let end = i + c.len_utf8();
                if self.buf[..end].trim().chars().count() >= MIN_SENTENCE_CHARS {
                    return Some(end);
                }
            }
        }
        None
    }
}

/// Vietnamese if the text uses letters that English never does.
pub fn is_vietnamese(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c, 'đ' | 'Đ' | 'ă' | 'Ă' | 'â' | 'Â' | 'ê' | 'Ê' | 'ô' | 'Ô' | 'ơ' | 'Ơ' | 'ư' | 'Ư')
            || ('\u{1EA0}'..='\u{1EF9}').contains(&c)
            || matches!(
                c,
                'à' | 'á'
                    | 'ả'
                    | 'ã'
                    | 'ạ'
                    | 'ì'
                    | 'í'
                    | 'ỉ'
                    | 'ĩ'
                    | 'ò'
                    | 'ó'
                    | 'ỏ'
                    | 'õ'
                    | 'ù'
                    | 'ú'
                    | 'ủ'
                    | 'ũ'
            )
    })
}

/// Removes what a TTS voice would read aloud badly: markdown markers, bullets, URLs.
pub fn speakable(text: &str) -> String {
    let words: Vec<&str> =
        text.split_whitespace().filter(|w| !(w.starts_with("http://") || w.starts_with("https://"))).collect();
    words
        .join(" ")
        .chars()
        .filter(|c| !matches!(c, '*' | '#' | '`' | '_' | '>' | '|'))
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_streamed_sentences() {
        let mut s = SentenceSplitter::default();
        assert!(s.push("Hà Nội là thủ đô").is_empty());
        assert_eq!(s.push(" của Việt Nam. Dân số kho"), vec!["Hà Nội là thủ đô của Việt Nam."]);
        assert!(s.push("ảng 8,5 triệu người").is_empty(), "the decimal comma and point do not split");
        assert_eq!(s.push(" vào năm 2024! Bạn"), vec!["Dân số khoảng 8,5 triệu người vào năm 2024!"]);
        assert_eq!(s.finish(), Some("Bạn".to_string()));
        assert_eq!(s.finish(), None);
    }

    #[test]
    fn short_fragments_wait_and_numbers_stay_whole() {
        let mut s = SentenceSplitter::default();
        assert!(s.push("Yes. Pi is 3.14 and ").is_empty(), "\"Yes.\" alone is too short");
        assert_eq!(s.push("more. "), vec!["Yes. Pi is 3.14 and more."]);
        assert_eq!(s.push("Line one\nLine"), vec!["Line one"]);
    }

    #[test]
    fn detects_vietnamese() {
        assert!(is_vietnamese("Xin chào"));
        assert!(is_vietnamese("Đúng vậy"));
        assert!(is_vietnamese("ừ"));
        assert!(!is_vietnamese("Hello there, how are you?"));
    }

    #[test]
    fn cleans_text_for_speech() {
        assert_eq!(speakable("**Bold** see https://x.y/z now"), "Bold see now");
        assert_eq!(speakable("# Title\n- item"), "Title - item");
    }
}
