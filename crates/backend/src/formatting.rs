//! Canonical local formatter entry points.
//!
//! Keep deterministic formatting here so labs, tests, and future runtime
//! integrations do not each grow their own number/email/path rules.

/// Number, currency, percent, storage, and duration formatting only.
pub fn apply_number_units(text: &str) -> String {
    crate::number_format::apply(text)
}

/// Structured-token formatting for emails, URLs, file paths, and identifiers.
pub fn apply_structured_tokens(text: &str) -> String {
    crate::llm::format_recover::recover(text)
}

/// Full local formatter used by the formatter lab and regression tests.
pub fn apply_all(text: &str) -> String {
    let numeric = apply_number_units(text);
    apply_structured_tokens(&numeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_structured_formatter_cases() {
        let cases = [
            (
                "Anish Suman 2305 at the rate gmail dot com",
                "anishsuman2305@gmail.com",
            ),
            (
                "anish suman two three zero five at the rate gmail dot com",
                "anishsuman2305@gmail.com",
            ),
            ("Mail anish at gmail dot com.", "Mail anish@gmail.com."),
            (
                "V abhi dot verma two six seven eight at the rate Gmail dot com.",
                "vabhi.verma2678@gmail.com.",
            ),
            (
                "rahul dot kumar at the rate yahoo dot com",
                "rahul.kumar@yahoo.com",
            ),
            ("support at emiac dot app", "support@emiac.app"),
            (
                "team at airnote dot app ko mail bhejo",
                "team@airnote.app ko mail bhejo",
            ),
            (
                "Open localhost colon 3000 slash api slash health.",
                "Open localhost:3000/api/health.",
            ),
            (
                "Open localhost colon three thousand slash api slash health.",
                "Open localhost:3000/api/health.",
            ),
            (
                "Check emiac dot app slash login slash callback.",
                "Check emiac.app/login/callback.",
            ),
            (
                "Run dot slash script slash dev dot sh please.",
                "Run ./script/dev.sh please.",
            ),
            (
                "Edit dot slash config slash said dot json file.",
                "Edit ./config/said.json file.",
            ),
            (
                "Set GATEWAY underscore API underscore KEY equals abc123.",
                "Set GATEWAY_API_KEY=abc123.",
            ),
            (
                "Check the slash api slash health endpoint.",
                "Check the /api/health endpoint.",
            ),
            (
                "Mail anish suman two three zero five at the rate gmail dot com aur twenty percent discount bhejo.",
                "Mail anishsuman2305@gmail.com aur 20% discount bhejo.",
            ),
            (
                "Invoice five hundred dollars ka hai aur mail finance at emiac dot app ko bhejo.",
                "Invoice $500 ka hai aur mail finance@emiac.app ko bhejo.",
            ),
        ];

        for (idx, (raw, expected)) in cases.iter().enumerate() {
            let got = apply_all(raw);
            println!("{:02}. {:?} => {:?}", idx + 1, raw, got);
            assert_eq!(got, *expected, "case {}", idx + 1);
        }
    }

    #[test]
    fn structured_formatter_safety_cases() {
        let cases = [
            "Growing at the rate of 10% every year.",
            "Add a slash at the end of the sentence.",
            "She used an underscore in her name.",
            "point of contact bhejo",
            "one on one meeting hai",
            "one and one meeting hai",
            "version one point release ready hai",
            "do log aaye",
            "five people joined",
        ];

        for raw in cases {
            let got = apply_all(raw);
            println!("{raw:?} => {got:?}");
            assert_eq!(got, raw);
        }
    }
}
