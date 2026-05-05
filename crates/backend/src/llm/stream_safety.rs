pub const STREAM_RESET_SENTINEL: &str = "\u{1F}__RESET__\u{1F}";

const PROMPT_LEAK_MARKERS: &[&str] = &[
    "<output_language>",
    "</output_language>",
    "<role>",
    "</role>",
    "<tone>",
    "</tone>",
    "<preferences>",
    "</preferences>",
    "<task>",
    "</task>",
    "<transcript>",
    "</transcript>",
    "<personal_vocabulary>",
    "</personal_vocabulary>",
    "<polish_preferences>",
    "</polish_preferences>",
    "ai produced:",
    "user changed it to:",
    "output only the final polished text",
    "the transcript remains the source of truth",
    "possible vocabulary hints.",
    "already matched in this transcript.",
    "confidence markers like [word?xx%]",
];

const META_PREFIXES: &[&str] = &[
    "polished text:",
    "final polished text:",
    "final answer:",
    "output:",
    "answer:",
    "transcript:",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamProvider {
    Codex,
    Groq,
    GeminiDirect,
    Gateway,
    Other,
}

impl StreamProvider {
    pub fn from_llm_provider(provider: &str) -> Self {
        match provider {
            "openai_codex" => Self::Codex,
            "groq" => Self::Groq,
            "gemini_direct" => Self::GeminiDirect,
            "gateway" => Self::Gateway,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct StreamFilterOutcome {
    pub tokens: Vec<String>,
    pub unsafe_detected: bool,
    pub live_disabled: bool,
}

#[derive(Debug)]
pub struct StreamSafetyFilter {
    provider: StreamProvider,
    transcript_norm: String,
    buffer: String,
    buffered_word_count: usize,
    buffering_active: bool,
    live_disabled: bool,
    unsafe_detected: bool,
    reset_emitted: bool,
}

impl StreamSafetyFilter {
    pub fn new(provider: StreamProvider, transcript: &str) -> Self {
        Self {
            provider,
            transcript_norm: normalize_for_compare(transcript),
            buffer: String::new(),
            buffered_word_count: 0,
            buffering_active: matches!(provider, StreamProvider::Groq),
            live_disabled: false,
            unsafe_detected: false,
            reset_emitted: false,
        }
    }

    pub fn saw_unsafe_content(&self) -> bool {
        self.unsafe_detected
    }

    pub fn live_disabled(&self) -> bool {
        self.live_disabled
    }

    pub fn push_token(&mut self, token: String) -> StreamFilterOutcome {
        if token == STREAM_RESET_SENTINEL {
            self.buffering_active = false;
            self.live_disabled = true;
            self.unsafe_detected = true;
            self.reset_emitted = true;
            return StreamFilterOutcome {
                tokens: vec![STREAM_RESET_SENTINEL.to_string()],
                unsafe_detected: true,
                live_disabled: true,
            };
        }

        if self.live_disabled {
            return StreamFilterOutcome {
                live_disabled: true,
                ..StreamFilterOutcome::default()
            };
        }

        if self.requires_early_buffer() {
            self.buffer.push_str(&token);
            self.buffered_word_count = self.buffer.split_whitespace().count();

            if is_prompt_leak(&self.buffer)
                || looks_like_transcript_echo(&self.buffer, &self.transcript_norm)
            {
                self.buffering_active = false;
                self.live_disabled = true;
                self.unsafe_detected = true;
                let mut tokens = Vec::new();
                if !self.reset_emitted {
                    tokens.push(STREAM_RESET_SENTINEL.to_string());
                    self.reset_emitted = true;
                }
                return StreamFilterOutcome {
                    tokens,
                    unsafe_detected: true,
                    live_disabled: true,
                };
            }

            if !self.buffer_ready_for_release() {
                return StreamFilterOutcome::default();
            }

            self.buffering_active = false;
            let release = std::mem::take(&mut self.buffer);
            return StreamFilterOutcome {
                tokens: vec![release],
                ..StreamFilterOutcome::default()
            };
        }

        StreamFilterOutcome {
            tokens: vec![token],
            ..StreamFilterOutcome::default()
        }
    }

    fn requires_early_buffer(&self) -> bool {
        self.buffering_active
            && matches!(self.provider, StreamProvider::Groq)
            && !self.live_disabled
    }

    fn buffer_ready_for_release(&self) -> bool {
        if self.buffer.is_empty() {
            return false;
        }
        if self.buffer.contains('\n') || ends_with_sentence_punctuation(&self.buffer) {
            return true;
        }
        self.buffer.len() >= 56 || self.buffered_word_count >= 8
    }
}

pub fn scrub_polished_output(text: &str, transcript: &str, prefer_suffix: bool) -> String {
    let cleaned = remove_tag_blocks(text);
    let segments = cleaned
        .lines()
        .filter_map(|line| scrub_line(line))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if segments.is_empty() {
        return condense_whitespace(text);
    }

    let transcript_norm = normalize_for_compare(transcript);
    let mut chosen = segments.join(" ");
    if prefer_suffix
        && segments.len() > 1
        && looks_like_transcript_echo(&segments[0], &transcript_norm)
    {
        chosen = segments
            .iter()
            .rev()
            .find(|segment| !looks_like_transcript_echo(segment, &transcript_norm))
            .cloned()
            .unwrap_or_else(|| segments.last().cloned().unwrap_or_default());
    }

    let chosen = condense_whitespace(&chosen);
    if prefer_suffix && looks_like_transcript_echo(&chosen, &transcript_norm) {
        let transcript_plain = condense_whitespace(transcript);
        if chosen.len() > transcript_plain.len() + 24
            && chosen
                .to_ascii_lowercase()
                .starts_with(&transcript_plain.to_ascii_lowercase())
        {
            let suffix = chosen[transcript_plain.len()..].trim_start_matches(|c: char| {
                c == ':' || c == '-' || c == '|' || c == '\n' || c.is_whitespace()
            });
            if !suffix.is_empty() {
                return suffix.to_string();
            }
        }
    }
    chosen
}

fn remove_tag_blocks(text: &str) -> String {
    let mut out = text.to_string();
    for marker in [
        "<output_language>",
        "</output_language>",
        "<role>",
        "</role>",
        "<tone>",
        "</tone>",
        "<preferences>",
        "</preferences>",
        "<task>",
        "</task>",
        "<transcript>",
        "</transcript>",
        "<personal_vocabulary>",
        "</personal_vocabulary>",
        "<polish_preferences>",
        "</polish_preferences>",
    ] {
        out = out.replace(marker, " ");
    }
    out
}

fn scrub_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || is_prompt_leak(trimmed) {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    for prefix in META_PREFIXES {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let offset = trimmed.len() - rest.len();
            let suffix = trimmed[offset..].trim();
            return if suffix.is_empty() {
                None
            } else {
                Some(suffix.to_string())
            };
        }
    }

    Some(trimmed.to_string())
}

fn is_prompt_leak(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    PROMPT_LEAK_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

fn looks_like_transcript_echo(text: &str, transcript_norm: &str) -> bool {
    if transcript_norm.is_empty() {
        return false;
    }
    let candidate = normalize_for_compare(text);
    if candidate.is_empty() {
        return false;
    }

    let candidate_words: Vec<&str> = candidate.split_whitespace().collect();
    let transcript_words: Vec<&str> = transcript_norm.split_whitespace().collect();
    if candidate_words.len() < 4 || transcript_words.len() < 4 {
        return false;
    }

    let mut prefix_matches = 0usize;
    for (left, right) in candidate_words.iter().zip(transcript_words.iter()) {
        if left == right {
            prefix_matches += 1;
        } else {
            break;
        }
    }

    (prefix_matches >= 4 && prefix_matches == candidate_words.len().min(transcript_words.len()))
        || (candidate.starts_with(transcript_norm) && candidate.len() >= 24)
}

fn normalize_for_compare(text: &str) -> String {
    text.split_whitespace()
        .map(|part| part.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-'))
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn condense_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ends_with_sentence_punctuation(text: &str) -> bool {
    text.trim_end()
        .chars()
        .last()
        .map(|ch| matches!(ch, '.' | '!' | '?' | ':' | ';'))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        STREAM_RESET_SENTINEL, StreamProvider, StreamSafetyFilter, looks_like_transcript_echo,
        scrub_polished_output,
    };

    #[test]
    fn groq_transcript_echo_triggers_reset_and_disable() {
        let mut filter = StreamSafetyFilter::new(
            StreamProvider::Groq,
            "schedule my meeting for 2 pm sorry for 5 pm",
        );
        let out = filter.push_token("schedule my meeting for 2 pm sorry".into());
        assert_eq!(out.tokens, vec![STREAM_RESET_SENTINEL.to_string()]);
        assert!(out.unsafe_detected);
        assert!(filter.live_disabled());
    }

    #[test]
    fn groq_prompt_leak_triggers_reset() {
        let mut filter = StreamSafetyFilter::new(StreamProvider::Groq, "hello there");
        let out = filter.push_token("<transcript>\nhello there\n</transcript>".into());
        assert_eq!(out.tokens, vec![STREAM_RESET_SENTINEL.to_string()]);
        assert!(filter.saw_unsafe_content());
    }

    #[test]
    fn clean_groq_output_releases_after_safe_window() {
        let mut filter = StreamSafetyFilter::new(
            StreamProvider::Groq,
            "schedule my meeting for 2 pm sorry for 5 pm",
        );
        let out = filter.push_token("Schedule my meeting for 5 pm.".into());
        assert_eq!(
            out.tokens,
            vec!["Schedule my meeting for 5 pm.".to_string()]
        );
        assert!(!filter.live_disabled());
    }

    #[test]
    fn codex_does_not_buffer_clean_tokens() {
        let mut filter = StreamSafetyFilter::new(StreamProvider::Codex, "hello");
        let out = filter.push_token("Hello".into());
        assert_eq!(out.tokens, vec!["Hello".to_string()]);
    }

    #[test]
    fn final_scrub_removes_prompt_and_prefers_suffix() {
        let scrubbed = scrub_polished_output(
            "<transcript>\nwhat is going on\n</transcript>\n\nPolished text: What is going on?",
            "what is going on",
            true,
        );
        assert_eq!(scrubbed, "What is going on?");
    }

    #[test]
    fn echo_detector_needs_substantial_prefix_match() {
        assert!(looks_like_transcript_echo(
            "schedule my meeting for 2 pm",
            "schedule my meeting for 2 pm sorry for 5 pm"
        ));
        assert!(!looks_like_transcript_echo(
            "Please schedule it for 5 pm",
            "schedule my meeting for 2 pm sorry for 5 pm"
        ));
    }
}
