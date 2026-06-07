use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldSafety {
    SafeEditable,
    ReplaceOnlyIfSelected,
    CopyOnly,
    SuppressVoice,
}

#[must_use]
pub fn classify_field(field_hint: &str, selected_text: &str) -> FieldSafety {
    let hint = field_hint.trim().to_ascii_lowercase();
    if hint.contains("secure")
        || hint.contains("password")
        || hint.contains("otp")
        || hint.contains("bank")
        || hint.contains("payment")
        || hint.contains("phone")
        || hint.contains("numeric")
    {
        return FieldSafety::SuppressVoice;
    }

    if hint.contains("unsupported") || hint.contains("rejects_keyboard") {
        return FieldSafety::CopyOnly;
    }

    if !selected_text.trim().is_empty() {
        return FieldSafety::ReplaceOnlyIfSelected;
    }

    FieldSafety::SafeEditable
}

#[must_use]
pub fn learning_allowed(field_hint: &str) -> bool {
    matches!(
        classify_field(field_hint, ""),
        FieldSafety::SafeEditable | FieldSafety::ReplaceOnlyIfSelected
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_sensitive_fields() {
        for hint in ["secure", "password", "otp", "banking", "phone_number"] {
            assert_eq!(classify_field(hint, ""), FieldSafety::SuppressVoice);
            assert!(!learning_allowed(hint));
        }
    }

    #[test]
    fn selected_text_requires_explicit_replace_policy() {
        assert_eq!(
            classify_field("multiline", "replace this"),
            FieldSafety::ReplaceOnlyIfSelected
        );
    }
}
