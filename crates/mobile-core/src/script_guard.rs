use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptGuardReport {
    pub contains_devanagari: bool,
    pub should_warn: bool,
}

#[must_use]
pub fn contains_devanagari(text: &str) -> bool {
    text.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c))
}

#[must_use]
pub fn inspect_roman_hinglish_output(text: &str, roman_required: bool) -> ScriptGuardReport {
    let contains_devanagari = contains_devanagari(text);
    ScriptGuardReport {
        contains_devanagari,
        should_warn: roman_required && contains_devanagari,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_devanagari_when_roman_hinglish_is_required() {
        let report = inspect_roman_hinglish_output("Kal meeting hai", true);
        assert!(!report.contains_devanagari);
        assert!(!report.should_warn);

        let report = inspect_roman_hinglish_output("Kal meeting है", true);
        assert!(report.contains_devanagari);
        assert!(report.should_warn);
    }
}
