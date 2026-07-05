//! Step-2 formatting pass — runs AFTER the main polish LLM.
//!
//! The main polish LLM handles content cleaning (fillers, grammar, vocab
//! substitution). This formatter handles PRESENTATION only: converting
//! spoken-form patterns to their written equivalents.
//!
//! Architecture:
//!   1. `needs_formatting()` — wide regex pre-check (< 1ms). Designed to
//!      have near-zero false negatives by catching ALL number words (English
//!      + Hindi romanized variants), email/URL markers, date/time/currency
//!      patterns. Ambiguous Hindi words (ek, do, teen, char, das, nau) only
//!      trigger when followed by a unit (sau, baje, rupaye, etc.) to avoid
//!      false positives on normal Hindi speech.
//!   2. `format()` — calls Scout with a formatting-only prompt. Returns
//!      JSON `{replace: true/false, replacements: [...]}`. The LLM makes
//!      the final decision; the regex is just a cheap gate to skip plain text.

use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use tracing::{info, warn};

static WIDE_FORMAT_GATE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        # ═══ EMAIL / URL ═══
        at \s+ the \s+ rate |
        dot \s+ (com|org|net|in|io|ai|co|dev|edu|gov) \b |
        \b at \s+ (gmail|yahoo|hotmail|outlook|proton|zoho) \b |
        \b double \s+ u \b |
        (colon \s+ )?slash \s+ slash |
        \b h \s+ t \s+ t \s+ p \b |

        # ═══ ENGLISH NUMBER WORDS (always trigger) ═══
        \b (zero|one|two|three|four|five|six|seven|eight|nine|ten|
            eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|
            eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|
            eighty|ninety|hundred|thousand|million|billion|trillion) \b |

        # ═══ HINDI UNAMBIGUOUS NUMBERS (always trigger) ═══
        # These words are clearly numbers — no other common meaning
        \b (paanch|panch|paach|chheh|chah|cheh|chhah|chhe|
            saat|aath|gyaarah|gyarah|baarah|barah|
            terah|chaudah|pandrah|solah|satrah|atharah|
            unees|bees|ikkees|baees|tees|
            pacchees|pachees|pachaas|pachpan|
            saath|sattar|assee|nabbe|
            dedh) \b |

        # ═══ HINDI SCALES (always trigger — indicate numbers) ═══
        \b (sau|hazaar|hazar|lakh|crore|arab|kharab) \b |

        # ═══ HINDI AMBIGUOUS + UNIT (context-aware) ═══
        # ek/do/teen/char/che/nau/das are common Hindi words
        # only trigger when followed by a number-scale or unit
        \b (ek|do|teen|char|chaar|che|nau|das)
           \s+ (sau|hazaar|hazar|lakh|crore|arab|kharab|
                baje|bajke|rupaye|rupay|rupees|
                percent|pratishat|saal|mahine|hafte) \b |

        # ═══ TIME MARKERS ═══
        \b baje \b |
        \b bajkar \b |
        \b saadhe \b |
        \b o'?clock \b |
        \b quarter \s+ (to|past) \b |
        \b half \s+ past \b |

        # ═══ DATE — month names (except 'may' which is ambiguous) ═══
        \b (january|february|march|april|june|july|august|
            september|october|november|december) \b |
        # 'may' only in date context
        \b (first|second|third|\d+\s*(st|nd|rd|th)) \s+ (of \s+)? may \b |

        # ═══ ORDINALS + MONTH ═══
        \b (first|second|third|fourth|fifth|sixth|seventh|eighth|ninth|
            tenth|eleventh|twelfth|thirteenth|fourteenth|fifteenth|sixteenth|
            seventeenth|eighteenth|nineteenth|twentieth|
            twenty \s+ (first|second|third|fourth|fifth|sixth|seventh|eighth|ninth)|
            thirtieth|thirty \s+ first) \s+ (of \s+)?
           (january|february|march|april|may|june|july|august|
            september|october|november|december) \b |

        # ═══ CURRENCY ═══
        \b (rupaye|rupay|rupees|dollars|pounds|euros) \b |

        # ═══ PERCENTAGE ═══
        \b (percent|pratishat) \b |

        # ═══ CODE TOKENS ═══
        \b underscore \b |
        \b hyphen \b |

        # ═══ PHONE: 4+ consecutive digit words ═══
        \b (zero|one|two|three|four|five|six|seven|eight|nine) \s+
           (zero|one|two|three|four|five|six|seven|eight|nine) \s+
           (zero|one|two|three|four|five|six|seven|eight|nine) \s+
           (zero|one|two|three|four|five|six|seven|eight|nine) \b
        ",
    )
    .unwrap()
});

pub fn needs_formatting(text: &str) -> bool {
    WIDE_FORMAT_GATE.is_match(text)
}

const FORMAT_PROMPT: &str = r##"You are a SURGICAL TEXT FORMATTER for voice dictation. You receive polished dictation text and decide whether any spoken-form patterns need converting to their written form.

YOUR JOB: Look at the text. If it contains spoken-form patterns that should be formatted (emails, URLs, numbers, dates, times, currency, phone numbers, code tokens), set "replace": true and provide the exact find/replace pairs. If the text is already clean and needs no formatting, set "replace": false.

PATTERNS TO FIND AND REPLACE:

EMAILS:
- "at the rate" → "@", "dot com/org/net" → ".com/.org/.net"
- Combine name + digits + domain: "anish suman two three zero five at the rate gmail dot com" → "anishsuman2305@gmail.com"
- "underscore" between email parts → "_"

URLS:
- "double u double u double u" → "www"
- "dot com/org/net" → ".com/.org/.net"
- "h t t p s colon slash slash" → "https://"
- "slash" between path segments → "/"

NUMBERS (English):
- "thirty two billion" → "32 billion", "twenty five hundred" → "2500"
- "three point five percent" → "3.5%"
- Digit sequences: "nine eight seven six" → "9876"

NUMBERS (Hindi/Hinglish romanized — spelling varies, recognize ALL variants):
- ek/1, do/2, teen/3, char/chaar/4, paanch/panch/5
- chheh/chah/cheh/che/6, saat/7, aath/aath/8, nau/9, das/10
- gyaarah/gyarah/11, baarah/barah/12, terah/13, chaudah/14, pandrah/15
- solah/16, satrah/17, atharah/18, unees/19, bees/20
- sau/100, hazaar/hazar/1000, lakh/100000, crore/10000000
- "paanch sau" → "500", "saat crore" → "7 crore", "ek lakh" → "1 lakh"
- "bees hazaar" → "20,000", "ek sau pacchees" → "125"

TIMES (Hindi):
- "X baje" = "X o'clock" — "saat baje" → "7 baje", "chah baje" → "6 baje", "das baje" → "10 baje"
- "saadhe X" = "X:30" — "saadhe saat" → "7:30"
- "X bajkar Y minute" → "X:Y"

TIMES (English):
- "eight thirty AM" → "8:30 AM", "quarter to five" → "4:45"
- "half past three" → "3:30"

DATES:
- "nineteenth may twenty twenty six" → "19th May 2026"
- "first of january" → "1st January"

PHONE NUMBERS:
- Long digit sequences (7+ spoken digits) → concatenated: "nine eight seven six five four three two one zero" → "9876543210"
- "plus ninety one" prefix → "+91"

CURRENCY:
- "five hundred rupees" → "Rs 500", "paanch sau rupaye" → "Rs 500"
- "fifty dollars" → "$50"

CODE TOKENS (tech context only):
- "underscore" → "_", "hyphen" → "-", "hash" → "#"

SAFETY RULES:
- Do NOT touch content words, names, brands, Hinglish phrases.
- If "at the rate" is NOT part of an email, skip it.
- If number words are part of a name ("seven seas", "one direction"), skip them.
- Do NOT change the meaning or rephrase anything.
- Only output find strings that EXACTLY match text in the input (case-insensitive).
- When unsure, skip — a missed format is better than a wrong one.

OUTPUT FORMAT — respond with ONLY this JSON object:
{"replace": true, "replacements": [{"find": "exact text from input", "replace": "formatted version"}]}

If NOTHING needs formatting:
{"replace": false, "replacements": []}

EXAMPLES:

Input: aneesh suman two three zero five at the rate Gmail dot com ko mail karo please
Output: {"replace": true, "replacements": [{"find": "aneesh suman two three zero five at the rate Gmail dot com", "replace": "aneeshsuman2305@gmail.com"}]}

Input: thirty two billion parameter wala model hai aur saat crore ka budget
Output: {"replace": true, "replacements": [{"find": "thirty two billion", "replace": "32 billion"}, {"find": "saat crore", "replace": "7 crore"}]}

Input: Anish Suman two three zero five at the rate Gmail dot com ko mail likhna hai chah baje ka.
Output: {"replace": true, "replacements": [{"find": "Anish Suman two three zero five at the rate Gmail dot com", "replace": "AnishSuman2305@gmail.com"}, {"find": "chah baje", "replace": "6 baje"}]}

Input: meeting nineteenth may twenty twenty six ko hai saat baje
Output: {"replace": true, "replacements": [{"find": "nineteenth may twenty twenty six", "replace": "19th May 2026"}, {"find": "saat baje", "replace": "7 baje"}]}

Input: paanch sau rupaye mein teen kg aam milte hain
Output: {"replace": true, "replacements": [{"find": "paanch sau rupaye", "replace": "Rs 500"}]}

Input: Hello bhai kaise ho
Output: {"replace": false, "replacements": []}

Input: Maine kaam kar liya hai.
Output: {"replace": false, "replacements": []}

Input: MACOBS ka stock price kya hai
Output: {"replace": false, "replacements": []}"##;

#[derive(Deserialize)]
struct GroqResponse {
    choices: Vec<GroqChoice>,
}
#[derive(Deserialize)]
struct GroqChoice {
    message: GroqMessage,
}
#[derive(Deserialize)]
struct GroqMessage {
    content: String,
}

#[derive(Deserialize, Debug)]
struct FormatterResponse {
    replace: bool,
    #[serde(default)]
    replacements: Vec<Replacement>,
}

#[derive(Deserialize, Debug)]
struct Replacement {
    find: String,
    replace: String,
}

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const FORMATTER_MODEL: &str = "meta-llama/llama-4-scout-17b-16e-instruct";

pub async fn format(client: &Client, groq_api_key: &str, text: &str) -> String {
    if groq_api_key.is_empty() || text.trim().is_empty() {
        return text.to_string();
    }

    let start = std::time::Instant::now();
    info!("[formatter] formatting pass on {} chars", text.len());

    let body = serde_json::json!({
        "model": FORMATTER_MODEL,
        "stream": false,
        "temperature": 0.0,
        "max_tokens": 512,
        "messages": [
            { "role": "system", "content": FORMAT_PROMPT },
            { "role": "user",   "content": text },
        ]
    });

    let resp = match client
        .post(GROQ_ENDPOINT)
        .header("Authorization", format!("Bearer {groq_api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!("[formatter] request failed: {e} — returning original");
            return text.to_string();
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        warn!("[formatter] HTTP {status} — returning original");
        return text.to_string();
    }

    let raw = match resp.json::<GroqResponse>().await {
        Ok(r) => r
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default(),
        Err(e) => {
            warn!("[formatter] parse error: {e} — returning original");
            return text.to_string();
        }
    };

    let ms = start.elapsed().as_millis();

    if raw.is_empty() {
        info!("[formatter] empty response ({ms}ms)");
        return text.to_string();
    }

    // Strip markdown code fences if the LLM wraps the JSON
    let json_str = raw
        .trim()
        .strip_prefix("```json")
        .or_else(|| raw.trim().strip_prefix("```"))
        .unwrap_or(raw.trim())
        .strip_suffix("```")
        .unwrap_or(raw.trim())
        .trim();

    // Parse as new format {replace, replacements}, fall back to old [array] format
    let (should_replace, replacements) = match serde_json::from_str::<FormatterResponse>(json_str) {
        Ok(resp) => (resp.replace, resp.replacements),
        Err(_) => {
            // Backward compat: try plain array format
            match serde_json::from_str::<Vec<Replacement>>(json_str) {
                Ok(arr) => (!arr.is_empty(), arr),
                Err(e) => {
                    warn!("[formatter] bad JSON ({ms}ms): {e} — raw={raw:?}");
                    return text.to_string();
                }
            }
        }
    };

    if !should_replace || replacements.is_empty() {
        info!("[formatter] no changes needed ({ms}ms, replace={should_replace})");
        return text.to_string();
    }

    // Apply replacements surgically — case-insensitive find, exact replace
    let mut result = text.to_string();
    for r in &replacements {
        let find = r.find.trim();
        let replace = r.replace.trim();
        if find.is_empty() || replace.is_empty() || find == replace {
            continue;
        }
        let lower_result = result.to_ascii_lowercase();
        let lower_find = find.to_ascii_lowercase();
        if let Some(pos) = lower_result.find(&lower_find) {
            let before = &result[..pos];
            let after = &result[pos + find.len()..];
            info!("[formatter] replace: {:?} → {:?}", find, replace);
            result = format!("{before}{replace}{after}");
        }
    }

    info!(
        "[formatter] applied {} replacement(s) in {ms}ms",
        replacements.len()
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_replacements(text: &str, replacements: Vec<Replacement>) -> String {
        let mut result = text.to_string();
        for r in &replacements {
            let find = r.find.trim();
            let replace = r.replace.trim();
            if find.is_empty() || replace.is_empty() || find == replace {
                continue;
            }
            let lower_result = result.to_ascii_lowercase();
            let lower_find = find.to_ascii_lowercase();
            if let Some(pos) = lower_result.find(&lower_find) {
                let before = &result[..pos];
                let after = &result[pos + find.len()..];
                result = format!("{before}{replace}{after}");
            }
        }
        result
    }

    #[test]
    fn surgical_replace_applies_correctly() {
        let text = "aneesh suman two three zero five at the rate Gmail dot com ko mail karo";
        let result = apply_replacements(
            text,
            vec![Replacement {
                find: "aneesh suman two three zero five at the rate Gmail dot com".to_string(),
                replace: "aneeshsuman2305@gmail.com".to_string(),
            }],
        );
        assert_eq!(result, "aneeshsuman2305@gmail.com ko mail karo");
    }

    #[test]
    fn surgical_replace_handles_multiple() {
        let text = "thirty two billion budget aur saat crore profit";
        let result = apply_replacements(
            text,
            vec![
                Replacement {
                    find: "thirty two billion".to_string(),
                    replace: "32 billion".to_string(),
                },
                Replacement {
                    find: "saat crore".to_string(),
                    replace: "7 crore".to_string(),
                },
            ],
        );
        assert_eq!(result, "32 billion budget aur 7 crore profit");
    }

    #[test]
    fn parse_new_format_replace_true() {
        let json =
            r#"{"replace": true, "replacements": [{"find": "saat baje", "replace": "7 baje"}]}"#;
        let resp: FormatterResponse = serde_json::from_str(json).unwrap();
        assert!(resp.replace);
        assert_eq!(resp.replacements.len(), 1);
        assert_eq!(resp.replacements[0].find, "saat baje");
    }

    #[test]
    fn parse_new_format_replace_false() {
        let json = r#"{"replace": false, "replacements": []}"#;
        let resp: FormatterResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.replace);
        assert!(resp.replacements.is_empty());
    }

    #[test]
    fn parse_new_format_missing_replacements_defaults_empty() {
        let json = r#"{"replace": false}"#;
        let resp: FormatterResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.replace);
        assert!(resp.replacements.is_empty());
    }

    #[test]
    fn fallback_to_old_array_format() {
        let json = r#"[{"find": "saat baje", "replace": "7 baje"}]"#;
        let result: Result<FormatterResponse, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let arr: Vec<Replacement> = serde_json::from_str(json).unwrap();
        assert_eq!(arr.len(), 1);
    }

    // ═══════════════════════════════════════════════════════════════
    //  WIDE REGEX GATE — must-trigger cases
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn gate_triggers_email() {
        assert!(needs_formatting("anish at the rate gmail dot com"));
        assert!(needs_formatting("send to rahul at gmail dot com"));
        assert!(needs_formatting("mera email id hai underscore wala"));
    }

    #[test]
    fn gate_triggers_url() {
        assert!(needs_formatting(
            "double u double u double u dot google dot com"
        ));
        assert!(needs_formatting("h t t p s colon slash slash api dot com"));
    }

    #[test]
    fn gate_triggers_english_numbers() {
        assert!(needs_formatting("thirty two billion parameter model"));
        assert!(needs_formatting("twenty five hundred users"));
        assert!(needs_formatting("three point five percent growth"));
        assert!(needs_formatting("I have five things to do"));
    }

    #[test]
    fn gate_triggers_hindi_unambiguous_numbers() {
        assert!(needs_formatting("paanch baje meeting hai"));
        assert!(needs_formatting("saat crore ka budget hai"));
        assert!(needs_formatting("chheh log aaye the"));
        assert!(needs_formatting("chah baje tak aa jaana"));
        assert!(needs_formatting("cheh baje free ho?"));
        assert!(needs_formatting("chhah baje nahin"));
        assert!(needs_formatting("gyaarah baje doctor ke paas"));
        assert!(needs_formatting("bees hazaar ka phone"));
        assert!(needs_formatting("dedh sau log the"));
        assert!(needs_formatting("pacchees log aaye"));
        assert!(needs_formatting("aath bajke paanch minute"));
    }

    #[test]
    fn gate_triggers_hindi_scales() {
        assert!(needs_formatting("sau rupaye de do"));
        assert!(needs_formatting("lakh rupaye chahiye"));
        assert!(needs_formatting("crore ka deal hai"));
        assert!(needs_formatting("hazaar log aaye"));
    }

    #[test]
    fn gate_triggers_hindi_ambiguous_with_unit() {
        assert!(needs_formatting("do sau rupaye ka aayega"));
        assert!(needs_formatting("ek lakh ka budget"));
        assert!(needs_formatting("teen baje meeting hai"));
        assert!(needs_formatting("char sau log"));
        assert!(needs_formatting("das hazaar dena hai"));
        assert!(needs_formatting("nau baje tak aa jaana"));
    }

    #[test]
    fn gate_triggers_time() {
        assert!(needs_formatting("meeting eight AM pe hai"));
        assert!(needs_formatting("saat baje aana"));
        assert!(needs_formatting("quarter to five baje"));
        assert!(needs_formatting("half past three ko milte"));
        assert!(needs_formatting("six o'clock pe"));
    }

    #[test]
    fn gate_triggers_currency() {
        assert!(needs_formatting("paanch sau rupaye mein"));
        assert!(needs_formatting("fifty dollars ka plan"));
        assert!(needs_formatting("rupees de do"));
    }

    #[test]
    fn gate_triggers_percentage() {
        assert!(needs_formatting("ten percent growth"));
        assert!(needs_formatting("pratishat badha hai"));
    }

    #[test]
    fn gate_triggers_phone() {
        assert!(needs_formatting(
            "call nine eight seven six five four three two one zero"
        ));
    }

    #[test]
    fn gate_triggers_date() {
        assert!(needs_formatting("nineteenth may ko meeting"));
        assert!(needs_formatting("first of january se policy lagu"));
        assert!(needs_formatting("march mein project shuru"));
    }

    // ═══════════════════════════════════════════════════════════════
    //  WIDE REGEX GATE — must-NOT-trigger cases (plain text)
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn gate_skips_plain_hinglish() {
        assert!(!needs_formatting("Hello bhai, kaise ho?"));
        assert!(!needs_formatting("Kitna kaam ho gaya?"));
        assert!(!needs_formatting("Maine kaam kar liya hai."));
        assert!(!needs_formatting("MACOBS ka stock price kya hai"));
        assert!(!needs_formatting(
            "Achha hai, wajah se hi itna sara kaam hai"
        ));
    }

    #[test]
    fn gate_skips_hindi_ambiguous_without_unit() {
        assert!(!needs_formatting("isko mail kar do achhe se"));
        assert!(!needs_formatting("ek baar phir se bolo"));
        assert!(!needs_formatting("teen baar check kiya"));
        assert!(!needs_formatting("char log the wahan"));
    }

    #[test]
    fn gate_skips_already_formatted() {
        assert!(!needs_formatting("Send to anish@gmail.com"));
        assert!(!needs_formatting("Meeting at 8:00 AM"));
        assert!(!needs_formatting("Budget is large"));
    }

    // ═══════════════════════════════════════════════════════════════
    //  USER'S EXACT EXAMPLES from the bug report
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn gate_covers_user_examples() {
        // These are the exact sentences the user tested
        assert!(!needs_formatting("Hello bhai, kaise ho?"));
        assert!(!needs_formatting("Kitna time ho gaya aapse mile hue?"));
        assert!(needs_formatting("Aaj chah baje free ho?"));
        assert!(needs_formatting("Abhishek Verma at the rate gmail dot com"));
        assert!(!needs_formatting("isko mail kar do achhe se"));
        assert!(needs_formatting(
            "chah baje nahin, six o'clock ya five forty"
        ));
        assert!(needs_formatting("n8n rupees ka hi aata hai"));
        assert!(needs_formatting("Two hundred rupees ka hi aayega"));
        assert!(needs_formatting("Do sau rupaye ka aayega"));
    }
}
