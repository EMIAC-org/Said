//! Deterministic spoken-number → digit normalization for the voice pipeline.
//!
//! v1 scope: cardinals, percentages, storage units, simple currency (unit words
//! preserved). Does not touch email/URL/date recovery — those stay in format_recover.

use std::collections::HashMap;

use once_cell::sync::Lazy;

static ENGLISH_ONES: Lazy<HashMap<&'static str, u64>> = Lazy::new(|| {
    HashMap::from([
        ("zero", 0),
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("six", 6),
        ("seven", 7),
        ("eight", 8),
        ("nine", 9),
        ("ten", 10),
        ("eleven", 11),
        ("twelve", 12),
        ("thirteen", 13),
        ("fourteen", 14),
        ("fifteen", 15),
        ("sixteen", 16),
        ("seventeen", 17),
        ("eighteen", 18),
        ("nineteen", 19),
    ])
});

static ENGLISH_TENS: Lazy<HashMap<&'static str, u64>> = Lazy::new(|| {
    HashMap::from([
        ("twenty", 20),
        ("thirty", 30),
        ("forty", 40),
        ("fifty", 50),
        ("sixty", 60),
        ("seventy", 70),
        ("eighty", 80),
        ("ninety", 90),
    ])
});

static HINDI_ONES: Lazy<HashMap<&'static str, u64>> = Lazy::new(|| {
    HashMap::from([
        ("ek", 1),
        ("do", 2),
        ("teen", 3),
        ("char", 4),
        ("chaar", 4),
        ("paanch", 5),
        ("panch", 5),
        ("paach", 5),
        ("chheh", 6),
        ("chah", 6),
        ("cheh", 6),
        ("chhe", 6),
        ("che", 6),
        ("saat", 7),
        ("aath", 8),
        ("nau", 9),
        ("das", 10),
        ("gyarah", 11),
        ("gyaarah", 11),
        ("barah", 12),
        ("baarah", 12),
        ("terah", 13),
        ("chaudah", 14),
        ("pandrah", 15),
        ("solah", 16),
        ("satrah", 17),
        ("atharah", 18),
        ("unees", 19),
        ("bees", 20),
        ("pachees", 25),
        ("pacchees", 25),
        ("tees", 30),
        ("tis", 30),
        ("chaalis", 40),
        ("chalis", 40),
        ("pachas", 50),
        ("pachaas", 50),
        ("pachpan", 55),
        ("sattar", 70),
        ("assee", 80),
        ("nabbe", 90),
    ])
});

static SCALES: Lazy<HashMap<&'static str, u64>> = Lazy::new(|| {
    HashMap::from([
        ("hundred", 100),
        ("thousand", 1_000),
        ("million", 1_000_000),
        ("billion", 1_000_000_000),
        ("sau", 100),
        ("soo", 100),
        ("hazaar", 1_000),
        ("hazar", 1_000),
        ("lakh", 100_000),
        ("lac", 100_000),
        ("crore", 10_000_000),
    ])
});

static NUMBER_ALIASES: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {
    HashMap::from([
        ("thrite", "thirty"),
        ("thiry", "thirty"),
        ("therty", "thirty"),
        ("tirty", "thirty"),
        ("fourty", "forty"),
        ("ninty", "ninety"),
        ("ninteen", "nineteen"),
        ("eigth", "eight"),
        ("twelwe", "twelve"),
    ])
});

static CURRENCY_UNITS: Lazy<[&'static str; 8]> = Lazy::new(|| {
    [
        "rupaye", "rupay", "rupees", "rupee", "inr", "dollars", "dollar", "usd",
    ]
});

/// Hindi words that look like numbers ONLY when followed by a currency/money word.
/// "saath" = "together/with" in normal speech, but "saath rupaye" = Rs 60.
static CURRENCY_ONLY_NUMBERS: Lazy<HashMap<&'static str, u64>> =
    Lazy::new(|| HashMap::from([("saath", 60)]));

static COUNT_UNITS: Lazy<[&'static str; 10]> = Lazy::new(|| {
    [
        "mahine", "mahina", "months", "month", "saal", "year", "years", "din", "day", "days",
    ]
});

static DATE_UNITS: Lazy<[&'static str; 4]> =
    Lazy::new(|| ["tareekh", "tarikh", "tariikh", "tarik"]);

/// Phrases where a number-looking word is not numeric intent — never convert.
static BLOCKLIST: Lazy<[&'static str; 12]> = Lazy::new(|| {
    [
        "do this",
        "ek baar",
        "teen baar",
        "char log",
        "mere sath",
        "one thing",
        "for me",
        "to go",
        "ek bar",
        "do bar",
        "teen bar",
        "char bar",
    ]
});

/// Apply deterministic number/unit normalization to `text`.
///
/// Also strips stray leading/trailing ellipsis artifacts some STT engines emit
/// ("...And jo speed hai") — a light edge-only cleanup, see
/// [`said_core::text::strip_edge_ellipses`].
pub fn apply(text: &str) -> String {
    if text.trim().is_empty() {
        return text.to_string();
    }
    let stripped = said_core::text::strip_edge_ellipses(text);
    let text = stripped.as_str();
    if text.is_empty() {
        return String::new();
    }
    let lower = text.to_ascii_lowercase();
    let blocked = blocked_spans(&lower);
    let words = tokenize_words(text);
    if words.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut last_byte = 0usize;
    let mut i = 0;
    while i < words.len() {
        let start_byte = words[i].start;
        if blocked
            .iter()
            .any(|(s, e)| start_byte >= *s && start_byte < *e)
        {
            i += 1;
            continue;
        }

        let replacement = try_decimal_sequence(text, &words, i)
            .or_else(|| try_percent_sequence(text, &words, i))
            .or_else(|| try_storage_sequence(text, &words, i))
            .or_else(|| try_currency_sequence(text, &words, i))
            .or_else(|| try_count_unit_sequence(text, &words, i))
            .or_else(|| try_compact_suffix_sequence(text, &words, i))
            .or_else(|| try_number_sequence(text, &words, i));

        if let Some((replacement, end_word)) = replacement {
            out.push_str(&text[last_byte..words[i].start]);
            out.push_str(&replacement);
            last_byte = words[end_word - 1].end;
            i = end_word;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last_byte..]);
    out
}

#[derive(Debug, Clone)]
struct Token {
    surface: String,
    norm: String,
    start: usize,
    end: usize,
}

fn tokenize_words(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut norm = String::new();
    let mut start = 0;
    let mut in_word = false;

    for (idx, ch) in text.char_indices() {
        if ch.is_alphanumeric() || ch == '%' {
            if !in_word {
                start = idx;
                in_word = true;
            }
            current.push(ch);
            norm.push(ch.to_ascii_lowercase());
        } else {
            if in_word {
                let normalized = canonical_number_norm(&norm);
                tokens.push(Token {
                    surface: current.clone(),
                    norm: normalized,
                    start,
                    end: idx,
                });
                current.clear();
                norm.clear();
                in_word = false;
            }
        }
    }
    if in_word {
        let normalized = canonical_number_norm(&norm);
        tokens.push(Token {
            surface: current,
            norm: normalized,
            start,
            end: text.len(),
        });
    }
    tokens
}

fn blocked_spans(lower: &str) -> Vec<(usize, usize)> {
    BLOCKLIST
        .iter()
        .flat_map(|phrase| {
            let mut start = 0;
            let mut spans = Vec::new();
            while let Some(pos) = lower[start..].find(phrase) {
                let abs = start + pos;
                spans.push((abs, abs + phrase.len()));
                start = abs + 1;
            }
            spans
        })
        .collect()
}

fn is_word(token: &Token) -> bool {
    !token.norm.is_empty()
}

fn try_number_sequence(text: &str, tokens: &[Token], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    let mut words = Vec::new();
    while i < tokens.len() {
        if i > start && !only_whitespace_between(text, tokens[i - 1].end, tokens[i].start) {
            break;
        }
        let w = tokens[i].norm.as_str();
        if can_use_number_connector(text, tokens, i, &words) {
            words.push(w);
            i += 1;
            continue;
        }
        if !is_number_word(w) {
            break;
        }
        words.push(w);
        i += 1;
    }
    if words.is_empty() {
        return None;
    }
    if i < tokens.len()
        && tokens[i].norm == "and"
        && words.iter().any(|word| SCALES.contains_key(*word))
    {
        return None;
    }
    if words.len() == 1 && i < tokens.len() && matches!(tokens[i].norm.as_str(), "and" | "or") {
        return None;
    }
    if words.len() == 1 {
        if let Some(v) = safe_standalone_number_word(words[0]) {
            return Some((v.to_string(), start + 1));
        }
        return None;
    }
    if !plain_number_sequence_allowed(&words) {
        return None;
    }

    if let Some(value) = parse_number_words(&words) {
        let formatted = readable_large_scale(&words, value).unwrap_or_else(|| value.to_string());
        return Some((formatted, i));
    }
    None
}

fn try_decimal_sequence(text: &str, tokens: &[Token], start: usize) -> Option<(String, usize)> {
    let mut point_idx = start;
    let mut whole_words = Vec::new();
    while point_idx < tokens.len() && is_word(&tokens[point_idx]) {
        if point_idx > start
            && !only_whitespace_between(text, tokens[point_idx - 1].end, tokens[point_idx].start)
        {
            return None;
        }
        let w = tokens[point_idx].norm.as_str();
        if w == "point" {
            break;
        }
        if can_use_number_connector(text, tokens, point_idx, &whole_words) {
            whole_words.push(w);
            point_idx += 1;
            continue;
        }
        if !is_number_word(w) {
            return None;
        }
        whole_words.push(w);
        point_idx += 1;
    }
    if whole_words.is_empty()
        || point_idx >= tokens.len()
        || tokens[point_idx].norm != "point"
        || point_idx + 1 >= tokens.len()
    {
        return None;
    }
    if !only_whitespace_between(text, tokens[point_idx - 1].end, tokens[point_idx].start)
        || !only_whitespace_between(text, tokens[point_idx].end, tokens[point_idx + 1].start)
    {
        return None;
    }
    let whole = parse_number_words(&whole_words)?;
    let (fraction, mut end_idx) = parse_fraction_words(text, tokens, point_idx + 1)?;
    let mut formatted = format!("{whole}.{fraction}");
    if end_idx < tokens.len()
        && only_whitespace_between(text, tokens[end_idx - 1].end, tokens[end_idx].start)
    {
        if let Some(scale) = SCALES.get(tokens[end_idx].norm.as_str()).copied() {
            if let Some(scaled) = scaled_decimal_value(whole, &fraction, scale) {
                formatted = scaled.to_string();
                end_idx += 1;
                if end_idx < tokens.len()
                    && only_whitespace_between(text, tokens[end_idx - 1].end, tokens[end_idx].start)
                    && is_currency_unit(&tokens[end_idx].norm)
                {
                    formatted = format!("{}{}", currency_symbol(&tokens[end_idx].norm), formatted);
                    end_idx += 1;
                }
                return Some((formatted, end_idx));
            }
        }
    }
    if end_idx < tokens.len()
        && only_whitespace_between(text, tokens[end_idx - 1].end, tokens[end_idx].start)
        && matches!(tokens[end_idx].norm.as_str(), "percent" | "pratishat")
    {
        formatted.push('%');
        end_idx += 1;
    }
    if end_idx < tokens.len()
        && only_whitespace_between(text, tokens[end_idx - 1].end, tokens[end_idx].start)
        && is_currency_unit(&tokens[end_idx].norm)
    {
        formatted = format!("{}{}", currency_symbol(&tokens[end_idx].norm), formatted);
        end_idx += 1;
    }
    Some((formatted, end_idx))
}

fn try_percent_sequence(text: &str, tokens: &[Token], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    let mut words = Vec::new();
    while i < tokens.len() && is_word(&tokens[i]) {
        if i > start && !only_whitespace_between(text, tokens[i - 1].end, tokens[i].start) {
            return None;
        }
        let w = tokens[i].norm.as_str();
        if w == "percent" || w == "pratishat" {
            break;
        }
        if can_use_number_connector(text, tokens, i, &words) {
            words.push(w);
            i += 1;
            continue;
        }
        if !is_number_word(w) {
            return None;
        }
        words.push(w);
        i += 1;
    }
    if i >= tokens.len() || !is_word(&tokens[i]) {
        return None;
    }
    let unit = tokens[i].norm.as_str();
    if unit != "percent" && unit != "pratishat" {
        return None;
    }
    let value = parse_number_words(&words)?;
    Some((format!("{value}%"), i + 1))
}

fn try_storage_sequence(text: &str, tokens: &[Token], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    let mut words = Vec::new();
    while i < tokens.len() && is_word(&tokens[i]) {
        if i > start && !only_whitespace_between(text, tokens[i - 1].end, tokens[i].start) {
            return None;
        }
        if let Some((unit, unit_end)) = storage_unit_at(text, tokens, i) {
            if words.is_empty() {
                return None;
            }
            let value = parse_compact_number(&words)?;
            return Some((format!("{value} {unit}"), unit_end));
        }
        let w = tokens[i].norm.as_str();
        if can_use_number_connector(text, tokens, i, &words) {
            words.push(w);
            i += 1;
            continue;
        }
        if !is_number_word(w) {
            return None;
        }
        words.push(w);
        i += 1;
    }
    None
}

fn try_currency_sequence(text: &str, tokens: &[Token], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    let mut words = Vec::new();
    while i < tokens.len() && is_word(&tokens[i]) {
        if i > start && !only_whitespace_between(text, tokens[i - 1].end, tokens[i].start) {
            return None;
        }
        let w = tokens[i].norm.as_str();
        if is_currency_unit(w) {
            if words.is_empty() {
                return None;
            }
            let value = parse_currency_number_words(&words)?;
            return Some((format!("{}{value}", currency_symbol(w)), i + 1));
        }
        if can_use_number_connector(text, tokens, i, &words) {
            words.push(w);
            i += 1;
            continue;
        }
        if CURRENCY_ONLY_NUMBERS.contains_key(w) {
            words.push(w);
            i += 1;
            continue;
        }
        if !is_number_word(w) {
            return None;
        }
        words.push(w);
        i += 1;
    }
    None
}

fn try_count_unit_sequence(text: &str, tokens: &[Token], start: usize) -> Option<(String, usize)> {
    let mut i = start;
    let mut words = Vec::new();
    while i < tokens.len() && is_word(&tokens[i]) {
        if i > start && !only_whitespace_between(text, tokens[i - 1].end, tokens[i].start) {
            return None;
        }
        let w = tokens[i].norm.as_str();
        if COUNT_UNITS.contains(&w) || DATE_UNITS.contains(&w) {
            if words.is_empty() {
                return None;
            }
            let value = parse_number_words(&words)?;
            return Some((format!("{value} {}", tokens[i].surface), i + 1));
        }
        if can_use_number_connector(text, tokens, i, &words) {
            words.push(w);
            i += 1;
            continue;
        }
        if !is_number_word(w) || words.len() >= 3 {
            return None;
        }
        words.push(w);
        i += 1;
    }
    None
}

fn try_compact_suffix_sequence(
    text: &str,
    tokens: &[Token],
    start: usize,
) -> Option<(String, usize)> {
    let mut i = start;
    let mut words = Vec::new();
    while i < tokens.len() && is_word(&tokens[i]) {
        if i > start && !only_whitespace_between(text, tokens[i - 1].end, tokens[i].start) {
            return None;
        }
        let w = tokens[i].norm.as_str();
        if w == "k" {
            if words.is_empty() {
                return None;
            }
            let value = parse_number_words(&words)?;
            return Some((format!("{value}k"), i + 1));
        }
        if can_use_number_connector(text, tokens, i, &words) {
            words.push(w);
            i += 1;
            continue;
        }
        if !is_number_word(w) {
            return None;
        }
        words.push(w);
        i += 1;
    }
    None
}

fn normalize_unit(unit: &str) -> String {
    unit.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn is_currency_unit(word: &str) -> bool {
    CURRENCY_UNITS.contains(&word)
}

fn can_use_number_connector(text: &str, tokens: &[Token], idx: usize, words: &[&str]) -> bool {
    if tokens[idx].norm != "and" {
        return false;
    }
    if !words.iter().any(|word| SCALES.contains_key(*word)) {
        return false;
    }
    idx + 1 < tokens.len()
        && only_whitespace_between(text, tokens[idx].end, tokens[idx + 1].start)
        && is_number_word(&tokens[idx + 1].norm)
}

fn canonical_number_norm(norm: &str) -> String {
    NUMBER_ALIASES
        .get(norm)
        .copied()
        .unwrap_or(norm)
        .to_string()
}

fn scaled_decimal_value(whole: u64, fraction: &str, scale: u64) -> Option<u64> {
    let numerator = fraction.parse::<u64>().ok()?;
    let denominator = 10u64.checked_pow(fraction.len().try_into().ok()?)?;
    let whole_scaled = whole.checked_mul(scale)?;
    let fraction_scaled_num = numerator.checked_mul(scale)?;
    if fraction_scaled_num % denominator != 0 {
        return None;
    }
    whole_scaled.checked_add(fraction_scaled_num / denominator)
}

fn currency_symbol(word: &str) -> &'static str {
    match word {
        "dollar" | "dollars" | "usd" => "$",
        _ => "Rs ",
    }
}

fn plain_number_sequence_allowed(words: &[&str]) -> bool {
    if words.len() == 2 {
        if ENGLISH_TENS.contains_key(words[0])
            && ENGLISH_ONES.get(words[1]).is_some_and(|v| *v < 10)
        {
            return true;
        }
        if single_number_word(words[0]).is_some() && SCALES.contains_key(words[1]) {
            return true;
        }
    }
    if words.len() == 3 {
        if english_digit_word(words[0]).is_some()
            && ENGLISH_TENS.contains_key(words[1])
            && english_digit_word(words[2]).is_some()
        {
            return true;
        }
    }
    words.iter().any(|word| SCALES.contains_key(*word))
}

fn parse_fraction_words(text: &str, tokens: &[Token], start: usize) -> Option<(String, usize)> {
    let first = tokens.get(start)?.norm.as_str();
    if first.chars().all(|c| c.is_ascii_digit()) {
        return Some((first.to_string(), start + 1));
    }

    if let Some(digit) = english_digit_word(first) {
        let mut digits = digit.to_string();
        let mut end = start + 1;
        while end < tokens.len()
            && only_whitespace_between(text, tokens[end - 1].end, tokens[end].start)
            && english_digit_word(&tokens[end].norm).is_some()
            && digits.len() < 4
        {
            digits.push_str(&english_digit_word(&tokens[end].norm)?.to_string());
            end += 1;
        }
        return Some((digits, end));
    }

    if let Some(value) = ENGLISH_ONES.get(first) {
        if *value >= 10 {
            return Some((format!("{value:02}"), start + 1));
        }
    }
    if let Some(tens) = ENGLISH_TENS.get(first) {
        if start + 1 < tokens.len()
            && only_whitespace_between(text, tokens[start].end, tokens[start + 1].start)
        {
            if let Some(ones) = ENGLISH_ONES.get(tokens[start + 1].norm.as_str()) {
                if *ones < 10 {
                    return Some((format!("{}", tens + ones), start + 2));
                }
            }
        }
        return Some((format!("{tens:02}"), start + 1));
    }

    None
}

fn storage_unit_at(text: &str, tokens: &[Token], idx: usize) -> Option<(String, usize)> {
    let unit = normalize_unit(&tokens[idx].norm);
    if matches!(unit.as_str(), "gb" | "mb" | "tb" | "kb") {
        return Some((unit.to_ascii_uppercase(), idx + 1));
    }
    if idx + 1 < tokens.len()
        && only_whitespace_between(text, tokens[idx].end, tokens[idx + 1].start)
    {
        let two_token = normalize_unit(&format!("{}{}", tokens[idx].norm, tokens[idx + 1].norm));
        if matches!(two_token.as_str(), "gb" | "mb" | "tb" | "kb") {
            return Some((two_token.to_ascii_uppercase(), idx + 2));
        }
    }
    None
}

fn single_number_word(word: &str) -> Option<u64> {
    if word.chars().all(|c| c.is_ascii_digit()) {
        return word.parse().ok();
    }
    ENGLISH_ONES
        .get(word)
        .or_else(|| ENGLISH_TENS.get(word))
        .or_else(|| HINDI_ONES.get(word))
        .copied()
}

fn safe_standalone_number_word(word: &str) -> Option<u64> {
    if word.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Some(v) = ENGLISH_TENS.get(word) {
        return Some(*v);
    }
    match word {
        "pachas" | "pachaas" | "pachpan" | "bees" | "tees" | "pacchees" | "pachees" | "sattar"
        | "assee" | "nabbe" | "hazaar" | "hazar" | "lakh" | "crore" => parse_number_words(&[word]),
        _ => None,
    }
}

fn is_number_word(word: &str) -> bool {
    word.chars().all(|c| c.is_ascii_digit())
        || ENGLISH_ONES.contains_key(word)
        || ENGLISH_TENS.contains_key(word)
        || HINDI_ONES.contains_key(word)
        || SCALES.contains_key(word)
}

fn only_whitespace_between(text: &str, start: usize, end: usize) -> bool {
    text[start..end].chars().all(char::is_whitespace)
}

fn parse_compact_number(words: &[&str]) -> Option<u64> {
    // "one twenty eight" → 128, "two three zero five" → 2305
    if words.len() == 3 {
        if let (Some(first), Some(tens), Some(ones)) = (
            english_digit_word(words[0]),
            ENGLISH_TENS.get(words[1]).copied(),
            english_digit_word(words[2]),
        ) {
            return Some(first * 100 + tens + ones);
        }
    }
    if words.len() >= 2 && words.iter().all(|w| english_digit_word(w).is_some()) {
        let mut digits = String::new();
        for w in words {
            digits.push_str(&english_digit_word(w)?.to_string());
        }
        return digits.parse().ok();
    }
    parse_number_words(words)
}

fn english_digit_word(word: &str) -> Option<u64> {
    if let Some(v) = ENGLISH_ONES.get(word) {
        if *v <= 9 {
            return Some(*v);
        }
    }
    if let Some(v) = HINDI_ONES.get(word) {
        if *v <= 9 {
            return Some(*v);
        }
    }
    None
}

/// Like `parse_number_words` but also recognizes `CURRENCY_ONLY_NUMBERS` (e.g. "saath" = 60).
/// Used exclusively inside `try_currency_sequence` where a trailing currency unit is guaranteed.
fn parse_currency_number_words(words: &[&str]) -> Option<u64> {
    if words.len() == 1 {
        if let Some(v) = CURRENCY_ONLY_NUMBERS.get(words[0]) {
            return Some(*v);
        }
    }
    // For multi-word sequences containing a currency-only word, substitute it
    // into the normal parser by resolving it first.
    if words.iter().any(|w| CURRENCY_ONLY_NUMBERS.contains_key(w)) {
        let resolved: Vec<&str> = words
            .iter()
            .map(|w| {
                if CURRENCY_ONLY_NUMBERS.contains_key(w) {
                    "sixty" // map to English equivalent for the generic parser
                } else {
                    *w
                }
            })
            .collect();
        return parse_number_words(&resolved);
    }
    parse_number_words(words)
}

/// True when `word` is a small Hindi number (not a scale like sau/hazaar).
/// Hindi never adds two small numbers: "do char" ≠ 6, "ek teen" ≠ 4.
fn is_small_hindi_number(word: &str) -> bool {
    HINDI_ONES.contains_key(word) && !SCALES.contains_key(word)
}

fn parse_number_words(words: &[&str]) -> Option<u64> {
    if words.is_empty() {
        return None;
    }

    // Two consecutive small Hindi numbers without a scale between them is never
    // a valid number. "do char sau" is "give Rs 400", not Rs 600. Hindi has
    // dedicated words for compound numbers (chheh=6, not do+char).
    for pair in words.windows(2) {
        if is_small_hindi_number(pair[0]) && is_small_hindi_number(pair[1]) {
            return None;
        }
    }

    // English tens + ones: "sixty eight"
    if words.len() == 2 {
        if let (Some(tens), Some(ones)) = (ENGLISH_TENS.get(words[0]), ENGLISH_ONES.get(words[1])) {
            if *ones < 10 {
                return Some(tens + ones);
            }
        }
        if let (Some(a), Some(b)) = (single_number_word(words[0]), SCALES.get(words[1])) {
            if *b == 100 || *b == 1_000 {
                return Some(a * b);
            }
        }
    }

    // Compact spoken forms: "one twenty eight" -> 128.
    if words.len() == 3 {
        if let (Some(first), Some(tens), Some(ones)) = (
            english_digit_word(words[0]),
            ENGLISH_TENS.get(words[1]).copied(),
            english_digit_word(words[2]),
        ) {
            return Some(first * 100 + tens + ones);
        }
    }

    // Hindi compound: "do sau", "paanch sau"
    if words.len() == 2 {
        if let (Some(a), Some(b)) = (
            HINDI_ONES
                .get(words[0])
                .or_else(|| ENGLISH_ONES.get(words[0])),
            SCALES.get(words[1]),
        ) {
            if *b == 100 || *b == 1_000 || *b == 100_000 || *b == 10_000_000 {
                return Some(a * b);
            }
        }
    }

    // Single scale word with implied one: "sau" alone unlikely; "hazaar" = 1000
    if words.len() == 1 {
        return single_number_word(words[0])
            .or_else(|| SCALES.get(words[0]).copied().filter(|&v| v >= 100));
    }

    // Grouped scale parser: "thirty one thousand", "one million five hundred thousand".
    let mut total: u64 = 0;
    let mut current: u64 = 0;
    let mut saw_number = false;

    for (idx, word) in words.iter().enumerate() {
        if *word == "and" {
            continue;
        }
        if word.chars().all(|c| c.is_ascii_digit()) {
            current += word.parse::<u64>().ok()?;
            saw_number = true;
        } else if let Some(v) = ENGLISH_ONES.get(word) {
            current += v;
            saw_number = true;
        } else if let Some(v) = ENGLISH_TENS.get(word) {
            current += v;
            saw_number = true;
        } else if let Some(v) = HINDI_ONES.get(word) {
            current += v;
            saw_number = true;
        } else if let Some(scale) = SCALES.get(word) {
            if current == 0 {
                current = 1;
            }
            if *scale == 100 {
                current = current.checked_mul(100)?;
            } else if next_scale_is_larger(words, idx, *scale) {
                current = current.checked_mul(*scale)?;
            } else {
                total = total.checked_add(current.checked_mul(*scale)?)?;
                current = 0;
            }
            saw_number = true;
        } else {
            return None;
        }
    }
    if !saw_number {
        return None;
    }
    Some(total + current)
}

fn readable_large_scale(words: &[&str], value: u64) -> Option<String> {
    if words.iter().any(|word| *word == "crore") && value >= 10_000_000 {
        return Some(format!("{} crore", format_crore_value(value)));
    }
    None
}

fn format_crore_value(value: u64) -> String {
    let crore = 10_000_000u64;
    if value % crore == 0 {
        return format_indian_compact_count(value / crore);
    }
    if value % 100_000 == 0 {
        return format_decimal_ratio(value, crore, 2);
    }
    format_indian_compact_count(value)
}

fn format_indian_compact_count(value: u64) -> String {
    if value >= 100_000 && value % 100_000 == 0 {
        return format!("{} lakh", value / 100_000);
    }
    value.to_string()
}

fn format_decimal_ratio(value: u64, denominator: u64, max_decimal_places: usize) -> String {
    let whole = value / denominator;
    let mut remainder = value % denominator;
    if remainder == 0 {
        return whole.to_string();
    }

    let mut digits = String::new();
    for _ in 0..max_decimal_places {
        remainder *= 10;
        digits.push(char::from(b'0' + (remainder / denominator) as u8));
        remainder %= denominator;
        if remainder == 0 {
            break;
        }
    }
    while digits.ends_with('0') {
        digits.pop();
    }
    if digits.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{digits}")
    }
}

fn next_scale_is_larger(words: &[&str], current_idx: usize, current_scale: u64) -> bool {
    words
        .iter()
        .skip(current_idx + 1)
        .find(|word| **word != "and")
        .and_then(|word| SCALES.get(*word).copied())
        .is_some_and(|next_scale| next_scale > current_scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hindi_cardinals() {
        assert_eq!(apply("pachas"), "50");
        assert_eq!(apply("pachaas"), "50");
        assert_eq!(apply("do sau"), "200");
        assert_eq!(apply("ek lakh"), "100000");
    }

    #[test]
    fn english_compounds() {
        assert_eq!(apply("sixty eight"), "68");
        assert_eq!(apply("twenty percent"), "20%");
        assert_eq!(apply("twenty dollars"), "$20");
        assert_eq!(apply("twenty five percent"), "25%");
        assert_eq!(apply("thirty one thousand users"), "31000 users");
        assert_eq!(apply("thrite one thousand users"), "31000 users");
        assert_eq!(
            apply("one thousand two hundred thirty four users"),
            "1234 users"
        );
    }

    #[test]
    fn indian_scale_chains() {
        let cases = [
            (
                "Aaj maine char hazar crore ki property di hai.",
                "Aaj maine 4000 crore ki property di hai.",
            ),
            (
                "Aaj maine chaar hazaar crore ki property di hai.",
                "Aaj maine 4000 crore ki property di hai.",
            ),
            (
                "four thousand crore ki property hai",
                "4000 crore ki property hai",
            ),
            (
                "one lakh crore ka market size hai",
                "1 lakh crore ka market size hai",
            ),
            (
                "one crore pachaas lakh ka revenue hai",
                "1.5 crore ka revenue hai",
            ),
            ("forty k users aaye", "40k users aaye"),
            ("40 k users aaye", "40k users aaye"),
        ];

        for (raw, expected) in cases {
            let got = apply(raw);
            println!("{raw:?} => {got:?}");
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn storage_units() {
        assert_eq!(apply("one twenty eight GB"), "128 GB");
        assert_eq!(apply("one twenty eight GB RAM hai"), "128 GB RAM hai");
        assert_eq!(apply("one twenty eight g b RAM hai"), "128 GB RAM hai");
    }

    #[test]
    fn currency_preserves_unit_words() {
        assert_eq!(apply("paanch sau rupaye"), "Rs 500");
        assert_eq!(
            apply("Chaar soo rupaye ka invoice bhej do"),
            "Rs 400 ka invoice bhej do"
        );
        assert_eq!(apply("paanch sau rupaye bhejo"), "Rs 500 bhejo");
        assert_eq!(apply("five hundred dollars ka invoice"), "$500 ka invoice");
        assert_eq!(
            apply("monthly 5 dollar dena padega"),
            "monthly $5 dena padega"
        );
    }

    #[test]
    fn negative_common_phrases() {
        for phrase in [
            "do this",
            "ek baar",
            "teen baar",
            "char log",
            "mere sath",
            "one thing",
            "for me",
            "to go",
        ] {
            assert_eq!(apply(phrase), phrase, "should not convert {phrase}");
        }
    }

    #[test]
    fn compound_verb_do_not_number() {
        // "bata do char sau rupaye" = "tell (me) Rs 400", NOT Rs 600
        assert_eq!(
            apply("bata do char sau rupaye chahiye"),
            "bata do Rs 400 chahiye"
        );
        assert_eq!(apply("bhej do paanch sau rupaye"), "bhej do Rs 500");
        assert_eq!(
            apply("kar do teen sau rupaye transfer"),
            "kar do Rs 300 transfer"
        );
        // But standalone "do sau rupaye" (200 rupees) still works
        assert_eq!(apply("do sau rupaye bhejo"), "Rs 200 bhejo");
        // "laga do bees percent discount"
        assert_eq!(
            apply("laga do bees percent discount"),
            "laga do 20% discount"
        );
    }

    #[test]
    fn pipeline_macops_example() {
        let raw = "macops ka pachas percent growth hai";
        let numeric = apply(raw);
        assert!(numeric.contains("50"), "expected 50 in {numeric:?}");
        assert!(numeric.contains("macops"), "should preserve macops");
    }

    #[test]
    fn pipeline_meac_example() {
        let raw = "meac ka sixty eight percent hai";
        let numeric = apply(raw);
        assert!(
            numeric.contains("68%") || numeric.contains("68 %"),
            "{numeric:?}"
        );
    }

    #[test]
    fn sentence_context() {
        let out = apply("macops ka pachas percent growth hai");
        assert_eq!(out, "macops ka 50% growth hai");
    }

    #[test]
    fn hinglish_raw_sentence_matrix() {
        let cases = [
            (
                "macops ka pachas percent growth hai",
                "macops ka 50% growth hai",
            ),
            ("meac ka sixty eight percent hai", "meac ka 68% hai"),
            (
                "one twenty eight GB RAM wala laptop chahiye",
                "128 GB RAM wala laptop chahiye",
            ),
            (
                "one twenty eight g b RAM wala laptop chahiye",
                "128 GB RAM wala laptop chahiye",
            ),
            ("paanch sau rupaye ka bill bhejo", "Rs 500 ka bill bhejo"),
            ("do sau rupaye pending hai", "Rs 200 pending hai"),
            (
                "sixty eight percent users active hain",
                "68% users active hain",
            ),
            ("pachaas percent ka discount hai", "50% ka discount hai"),
            ("ek lakh users ka target hai", "100000 users ka target hai"),
            (
                "thirty one thousand users ka target hai",
                "31000 users ka target hai",
            ),
            (
                "thrite one thousand users ka target hai",
                "31000 users ka target hai",
            ),
            ("do this for me", "do this for me"),
            ("ek baar check karna", "ek baar check karna"),
            ("one thing batao", "one thing batao"),
        ];

        for (raw, expected) in cases {
            let got = apply(raw);
            println!("{raw:?} => {got:?}");
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn decimal_currency_and_duration_cases() {
        let cases = [
            (
                "Hello bhai, kaise ho?, Yeh to batao kitna kaam ho gaya., Tum kuchh batate hi nahi ho, na? Yahhi to dikkat hai., Chaar soo rupaye ka invoice bhej do, main use clear karwa dunga achhe se, aaj ke aaj baarah tariikh tak ho jayega kaam aapka.",
                "Hello bhai, kaise ho?, Yeh to batao kitna kaam ho gaya., Tum kuchh batate hi nahi ho, na? Yahhi to dikkat hai., Rs 400 ka invoice bhej do, main use clear karwa dunga achhe se, aaj ke aaj 12 tariikh tak ho jayega kaam aapka.",
            ),
            (
                "Aur agar yearly dete hain to one point nine nine baarah mahine ke hisaab se jo bhi amount hai us par bhi bees percent off ho jayega. Yeh total hisaab kitaab hai na?",
                "Aur agar yearly dete hain to 1.99 12 mahine ke hisaab se jo bhi amount hai us par bhi 20% off ho jayega. Yeh total hisaab kitaab hai na?",
            ),
            (
                "monthly five dollar dena padega aur yearly lene par twenty percent off hai",
                "monthly $5 dena padega aur yearly lene par 20% off hai",
            ),
            (
                "monthly 5 dollar dena padega aur yearly lene par 20 percent off hai",
                "monthly $5 dena padega aur yearly lene par 20% off hai",
            ),
            ("500 dollars ka invoice bana do", "$500 ka invoice bana do"),
            ("500 dollar ka invoice bana do", "$500 ka invoice bana do"),
            ("500 rupees ka bill bhejo", "Rs 500 ka bill bhejo"),
            ("500 rupaye ka bill bhejo", "Rs 500 ka bill bhejo"),
            (
                "one point eighteen dollar per month hai",
                "$1.18 per month hai",
            ),
            (
                "nineteen point ninety nine dollars yearly plan mein hai",
                "$19.99 yearly plan mein hai",
            ),
            (
                "baarah mahine ka total calculate karna",
                "12 mahine ka total calculate karna",
            ),
            (
                "aaj ke aaj baarah tariikh tak ho jayega",
                "aaj ke aaj 12 tariikh tak ho jayega",
            ),
            (
                "barah tareekh tak kaam ho jayega",
                "12 tareekh tak kaam ho jayega",
            ),
            ("20 percent off ho jayega", "20% off ho jayega"),
            ("50 percent ka discount hai", "50% ka discount hai"),
            ("200 rupaye pending hai", "Rs 200 pending hai"),
            (
                "thirty one thousand dollars pending hai",
                "$31000 pending hai",
            ),
            ("2 million dollars pending hai", "$2000000 pending hai"),
            ("two million dollars pending hai", "$2000000 pending hai"),
            (
                "one million five hundred thousand dollars pending hai",
                "$1500000 pending hai",
            ),
            (
                "ek lakh pachaas hazaar users ka target hai",
                "150000 users ka target hai",
            ),
        ];

        for (raw, expected) in cases {
            let got = apply(raw);
            println!("{raw:?} => {got:?}");
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn broad_hinglish_formatter_safety_corpus() {
        let cases = [
            // Percentages and decimals.
            ("pachas percent growth hai", "50% growth hai"),
            ("pachaas percent growth hai", "50% growth hai"),
            ("bees percent off hai", "20% off hai"),
            ("twenty percent off hai", "20% off hai"),
            ("ninety nine percent uptime hai", "99% uptime hai"),
            ("ninty nine percent uptime hai", "99% uptime hai"),
            ("one hundred percent complete hai", "100% complete hai"),
            ("one hundred and ten percent effort hai", "110% effort hai"),
            ("zero point five percent churn hai", "0.5% churn hai"),
            ("one point five percent fee hai", "1.5% fee hai"),
            ("20 percent off ho jayega", "20% off ho jayega"),
            // Currency, scales, and mixed digit/word amounts.
            ("one million dollars ka deal hai", "$1000000 ka deal hai"),
            ("1 million dollars ka deal hai", "$1000000 ka deal hai"),
            (
                "two billion dollars valuation hai",
                "$2000000000 valuation hai",
            ),
            (
                "2 billion dollars valuation hai",
                "$2000000000 valuation hai",
            ),
            (
                "one point five million dollars raise hai",
                "$1500000 raise hai",
            ),
            (
                "one point two billion dollars valuation hai",
                "$1200000000 valuation hai",
            ),
            (
                "zero point five million dollars budget hai",
                "$500000 budget hai",
            ),
            (
                "two point five lakh rupees pending hain",
                "Rs 250000 pending hain",
            ),
            (
                "one point five crore rupees ka revenue hai",
                "Rs 15000000 ka revenue hai",
            ),
            ("five hundred dollars ka invoice hai", "$500 ka invoice hai"),
            ("five hundred rupees ka bill hai", "Rs 500 ka bill hai"),
            ("twenty dollar ka plan hai", "$20 ka plan hai"),
            (
                "thirty one thousand dollars pending hai",
                "$31000 pending hai",
            ),
            (
                "one million and five hundred thousand dollars pending hai",
                "$1500000 pending hai",
            ),
            (
                "one hundred and twenty eight dollars charge hua",
                "$128 charge hua",
            ),
            // Counts and Indian/English scale phrases.
            (
                "thirty one thousand users ka target hai",
                "31000 users ka target hai",
            ),
            (
                "one thousand two hundred thirty four users aaye",
                "1234 users aaye",
            ),
            (
                "one hundred and twenty eight users active hain",
                "128 users active hain",
            ),
            (
                "one lakh pachaas hazaar users ka target hai",
                "150000 users ka target hai",
            ),
            (
                "ek lakh pachaas hazaar users ka target hai",
                "150000 users ka target hai",
            ),
            (
                "two million users onboard ho gaye",
                "2000000 users onboard ho gaye",
            ),
            (
                "one billion requests process hue",
                "1000000000 requests process hue",
            ),
            (
                "2 million requests process hue",
                "2000000 requests process hue",
            ),
            (
                "thrite one thousand users ka target hai",
                "31000 users ka target hai",
            ),
            ("fourty two users active hain", "42 users active hain"),
            // Storage and units.
            (
                "one twenty eight GB RAM wala laptop hai",
                "128 GB RAM wala laptop hai",
            ),
            (
                "one hundred and twenty eight GB storage hai",
                "128 GB storage hai",
            ),
            ("five hundred twelve GB SSD hai", "512 GB SSD hai"),
            ("two TB backup chahiye", "2 TB backup chahiye"),
            ("sixteen GB RAM hai", "16 GB RAM hai"),
            ("1 TB drive hai", "1 TB drive hai"),
            // Durations/count units.
            ("baarah mahine ka contract hai", "12 mahine ka contract hai"),
            ("baarah tariikh tak bhejna", "12 tariikh tak bhejna"),
            ("twenty four months ka plan hai", "24 months ka plan hai"),
            (
                "one hundred and twenty days ka window hai",
                "120 days ka window hai",
            ),
            ("teen mahine ka runway hai", "3 mahine ka runway hai"),
            // Safety: these should stay untouched unless a future feature explicitly supports them.
            ("do this for me", "do this for me"),
            ("one thing batao", "one thing batao"),
            ("ek baar check karna", "ek baar check karna"),
            ("char log meeting mein aaye", "char log meeting mein aaye"),
            ("teen baar try karna", "teen baar try karna"),
            ("do baar mat bolna", "do baar mat bolna"),
            ("one on one meeting hai", "one on one meeting hai"),
            ("one to one sync hai", "one to one sync hai"),
            ("one and one meeting hai", "one and one meeting hai"),
            (
                "version one point release ready hai",
                "version one point release ready hai",
            ),
            ("point of contact bhejo", "point of contact bhejo"),
            (
                "thirty and five weird phrase hai",
                "thirty and five weird phrase hai",
            ),
            (
                "one hundred and something users honge",
                "one hundred and something users honge",
            ),
            ("do options hain", "do options hain"),
            ("do log aaye", "do log aaye"),
            ("teen log aaye", "teen log aaye"),
            ("paanch log aaye", "paanch log aaye"),
            ("five people joined", "five people joined"),
            ("one file kholo", "one file kholo"),
            ("two files open karo", "two files open karo"),
            (
                "macobs one point release hai",
                "macobs one point release hai",
            ),
            // "saath" = together/with by default, number only with currency
            (
                "main tere saath office jaaunga",
                "main tere saath office jaaunga",
            ),
            ("uske saath kaam karo", "uske saath kaam karo"),
            ("saath rupaye dena", "Rs 60 dena"),
            ("saath rupay ka tha", "Rs 60 ka tha"),
            ("saath dollars lagenge", "$60 lagenge"),
        ];

        assert!(
            (50..=75).contains(&cases.len()),
            "expected a focused 50-75 case corpus, got {}",
            cases.len()
        );

        for (idx, (raw, expected)) in cases.iter().enumerate() {
            let got = apply(raw);
            println!("{:02}. {:?} => {:?}", idx + 1, raw, got);
            assert_eq!(got, *expected, "case {}", idx + 1);
        }
    }
}
