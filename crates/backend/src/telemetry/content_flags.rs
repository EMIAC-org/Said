//! Privacy-safe content flags derived from text at pipeline time (never persisted).

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ContentFlags {
    pub has_numbers: bool,
    pub has_currency: bool,
    pub has_percent: bool,
    pub has_email: bool,
    pub has_url: bool,
    pub has_code_like_terms: bool,
    pub mixed_language: bool,
    pub protected_term_hit: bool,
}

pub fn derive_content_flags(text: &str) -> ContentFlags {
    let t = text.trim();
    if t.is_empty() {
        return ContentFlags::default();
    }

    let has_numbers = t.chars().any(|c| c.is_ascii_digit());
    let has_currency = t.contains('₹')
        || t.contains('$')
        || t.contains('€')
        || t.contains('£')
        || t.to_ascii_lowercase().contains(" rupee")
        || t.to_ascii_lowercase().contains(" dollar");
    let has_percent = t.contains('%');
    let has_email = t.contains('@') && t.contains('.');
    let has_url = t.contains("http://")
        || t.contains("https://")
        || t.contains("www.")
        || t.contains(".com")
        || t.contains(".io");
    let has_code_like_terms = t.contains('_')
        || t.contains("()")
        || t.contains("->")
        || t.contains("API")
        || t.contains("SQL");
    let has_devanagari = t.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c));
    let has_latin = t.chars().any(|c| c.is_ascii_alphabetic());
    let mixed_language = has_devanagari && has_latin;
    let protected_term_hit = has_url || has_email || has_currency;

    ContentFlags {
        has_numbers,
        has_currency,
        has_percent,
        has_email,
        has_url,
        has_code_like_terms,
        mixed_language,
        protected_term_hit,
    }
}

pub fn edit_bucket_from_diff(polished: &str, kept: &str) -> (bool, &'static str, i32, i32) {
    let p = polished.trim();
    let k = kept.trim();
    if p == k || (p.is_empty() && k.is_empty()) {
        return (false, "none", 0, 0);
    }
    if k.is_empty() && !p.is_empty() {
        return (
            true,
            "full_replace",
            p.len() as i32,
            p.split_whitespace().count() as i32,
        );
    }
    if p.is_empty() && !k.is_empty() {
        return (
            true,
            "deleted",
            k.len() as i32,
            k.split_whitespace().count() as i32,
        );
    }

    let char_dist = (p.len() as i32 - k.len() as i32).unsigned_abs() as i32;
    let p_words: Vec<_> = p.split_whitespace().collect();
    let k_words: Vec<_> = k.split_whitespace().collect();
    let word_dist = (p_words.len() as i32 - k_words.len() as i32).unsigned_abs() as i32;

    let bucket = if word_dist <= 1 && char_dist <= 12 {
        "minor"
    } else if word_dist <= 3 {
        "small_phrase"
    } else if word_dist <= 8 {
        "medium"
    } else if p_words.len() > 0
        && k_words.len() > 0
        && word_dist as f32 / p_words.len().max(k_words.len()) as f32 > 0.6
    {
        "full_replace"
    } else {
        "heavy"
    };

    (true, bucket, char_dist, word_dist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_edit_bucket_for_identical_text() {
        let (edit, bucket, _, _) = edit_bucket_from_diff("hello world", "hello world");
        assert!(!edit);
        assert_eq!(bucket, "none");
    }

    #[test]
    fn minor_edit_one_word() {
        let (edit, bucket, _, _) = edit_bucket_from_diff("hello world", "hello there");
        assert!(edit);
        assert_eq!(bucket, "minor");
    }

    #[test]
    fn flags_detect_email_without_storing_text() {
        let f = derive_content_flags("reach me at a@b.com");
        assert!(f.has_email);
        assert!(f.protected_term_hit);
    }
}
