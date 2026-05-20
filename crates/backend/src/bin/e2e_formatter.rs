//! End-to-end formatter test harness.
//!
//! Tests the Step-2 formatting pass in isolation: given polished text with
//! spoken-form patterns, does the formatter LLM produce correct find/replace
//! pairs, and does the surgical replacement logic apply them correctly?
//!
//! The formatter LLM decides whether formatting is needed (no regex gate).
//! It returns {replace: true/false, replacements: [...]}.
//!
//! Usage:
//!   GROQ_API_KEY=gsk_... cargo run -p said-backend --bin e2e-formatter

use said_backend::llm::format_pass;
use std::time::Instant;

struct TestCase {
    id: &'static str,
    input: &'static str,
    must_contain: Vec<&'static str>,
    must_not_contain: Vec<&'static str>,
}

#[tokio::main]
async fn main() {
    let api_key = std::env::var("GROQ_API_KEY")
        .or_else(|_| std::env::var("GATEWAY_API_KEY"))
        .unwrap_or_default();

    if api_key.is_empty() {
        println!("SKIP: no GROQ_API_KEY set");
        return;
    }

    let client = reqwest::Client::new();
    let cases = build_test_cases();

    println!("══ FORMATTER E2E — {} test cases ══\n", cases.len());

    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for (idx, tc) in cases.iter().enumerate() {
        if idx > 0 && idx % 8 == 0 {
            println!("  ... pausing 5s for rate limit ...");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }

        // 1. Verify regex gate triggers for cases that expect changes
        let triggers = format_pass::needs_formatting(tc.input);
        let has_expected_change = !tc.must_contain.is_empty()
            && tc.must_contain.iter().any(|m| !tc.input.contains(m));

        if has_expected_change && !triggers {
            println!("  FAIL  {}: regex gate did NOT trigger", tc.id);
            println!("        input: {:?}", truncate(tc.input, 80));
            fail += 1;
            failures.push(format!("{}: regex gate missed", tc.id));
            continue;
        }

        // 2. Call formatter LLM
        let t0 = Instant::now();
        let output = format_pass::format(&client, &api_key, tc.input).await;
        let ms = t0.elapsed().as_millis();

        // 3. Check must_contain
        let mut case_ok = true;
        for &m in &tc.must_contain {
            if !output.contains(m) {
                println!("  FAIL  {}: missing {:?} ({ms}ms)", tc.id, m);
                println!("        input:  {:?}", truncate(tc.input, 80));
                println!("        output: {:?}", truncate(&output, 80));
                fail += 1;
                failures.push(format!("{}: missing {m:?}", tc.id));
                case_ok = false;
                break;
            }
        }
        if !case_ok {
            continue;
        }

        // Check must_not_contain
        for &m in &tc.must_not_contain {
            if output.contains(m) {
                println!("  FAIL  {}: should NOT contain {:?} ({ms}ms)", tc.id, m);
                println!("        input:  {:?}", truncate(tc.input, 80));
                println!("        output: {:?}", truncate(&output, 80));
                fail += 1;
                failures.push(format!("{}: unwanted {m:?}", tc.id));
                case_ok = false;
                break;
            }
        }
        if !case_ok {
            continue;
        }

        println!("  PASS  {} ({ms}ms)", tc.id);
        pass += 1;
    }

    println!("\n{}", "=".repeat(60));
    println!("  RESULTS: {} passed, {} failed / {} total", pass, fail, cases.len());
    println!("{}", "=".repeat(60));

    if !failures.is_empty() {
        println!("\nFAILURES:\n");
        for f in &failures {
            println!("  {f}");
        }
    }

    println!("\n{}", if fail == 0 { "PASS" } else { "FAIL" });
    if fail > 0 {
        std::process::exit(1);
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

fn build_test_cases() -> Vec<TestCase> {
    vec![
        // ═══════════════════════════════════════════════════════════
        //  EMAILS
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "E1-basic-email",
            input: "anish at the rate gmail dot com",
            must_contain: vec!["anish@gmail.com"],
            must_not_contain: vec!["at the rate", "dot com"],
        },
        TestCase {
            id: "E2-email-with-digits",
            input: "anish suman two three zero five at the rate gmail dot com",
            must_contain: vec!["anishsuman2305@gmail.com"],
            must_not_contain: vec!["at the rate"],
        },
        TestCase {
            id: "E3-email-dot-name",
            input: "rahul dot kumar at the rate yahoo dot com",
            must_contain: vec!["@yahoo.com"],
            must_not_contain: vec!["at the rate"],
        },
        TestCase {
            id: "E4-email-mid-sentence",
            input: "anish at the rate gmail dot com pe mail karo aur document attach karo",
            must_contain: vec!["anish@gmail.com", "mail karo", "document attach karo"],
            must_not_contain: vec!["at the rate"],
        },
        TestCase {
            id: "E5-two-emails",
            input: "pehle anish at the rate gmail dot com ko mail karo phir rahul dot sharma at the rate outlook dot com ko bhi",
            must_contain: vec!["anish@gmail.com", "outlook.com"],
            must_not_contain: vec![],
        },
        TestCase {
            id: "E6-false-positive-rate",
            input: "I rate this product at the rate of ten per hour",
            must_contain: vec!["rate this product"],
            must_not_contain: vec!["@"],
        },
        TestCase {
            id: "E7-dot-com-not-email",
            input: "company dot com pe jaake signup karo",
            must_contain: vec![".com"],
            must_not_contain: vec!["@"],
        },
        TestCase {
            id: "E8-underscore-email",
            input: "mera email id hai anish underscore suman at the rate gmail dot com",
            must_contain: vec!["anish_suman@gmail.com"],
            must_not_contain: vec!["at the rate"],
        },

        // ═══════════════════════════════════════════════════════════
        //  URLS
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "U1-www-url",
            input: "double u double u double u dot google dot com slash search",
            must_contain: vec!["www.google.com"],
            must_not_contain: vec!["double u"],
        },
        TestCase {
            id: "U2-https-url",
            input: "h t t p s colon slash slash api dot groq dot com slash v one slash chat",
            must_contain: vec!["https://", "groq"],
            must_not_contain: vec!["colon slash slash"],
        },
        TestCase {
            id: "U3-localhost",
            input: "localhost colon three thousand pe server chal raha hai",
            must_contain: vec!["localhost", "server chal raha hai"],
            must_not_contain: vec![],
        },
        TestCase {
            id: "U4-github-url",
            input: "github dot com slash emiac dash org slash said dekhlo",
            must_contain: vec!["github.com", "said"],
            must_not_contain: vec![],
        },
        TestCase {
            id: "U5-dot-com-destination",
            input: "dot com pe jaake check karo apna account",
            must_contain: vec![".com", "check karo"],
            must_not_contain: vec!["@"],
        },

        // ═══════════════════════════════════════════════════════════
        //  NUMBERS
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "N1-billion",
            input: "thirty two billion parameter wala model kaafi fast hai",
            must_contain: vec!["32 billion", "fast hai"],
            must_not_contain: vec!["thirty two"],
        },
        TestCase {
            id: "N2-crore",
            input: "saat crore ka budget approve hua hai",
            must_contain: vec!["7 crore", "budget"],
            must_not_contain: vec!["saat crore"],
        },
        TestCase {
            id: "N3-lakh-hazaar",
            input: "paanch lakh bees hazaar rupaye chahiye mujhe",
            must_contain: vec!["chahiye mujhe"],
            must_not_contain: vec!["paanch lakh bees hazaar"],
        },
        TestCase {
            id: "N4-decimal-percent",
            input: "three point five percent annual growth report mein dikhao",
            must_contain: vec!["3.5%", "growth"],
            must_not_contain: vec!["three point five"],
        },
        TestCase {
            id: "N5-hindi-compound",
            input: "ek sau pacchees log aaye the conference mein",
            must_contain: vec!["125", "conference"],
            must_not_contain: vec![],
        },
        TestCase {
            id: "N6-english-compound",
            input: "twenty five hundred users ne signup kiya hai",
            must_contain: vec!["2500", "signup"],
            must_not_contain: vec!["twenty five hundred"],
        },
        TestCase {
            id: "N7-false-positive-seven-seas",
            input: "seven seas restaurant mein dinner karte hain",
            must_contain: vec!["seven seas", "dinner"],
            must_not_contain: vec!["7 seas"],
        },
        TestCase {
            id: "N8-false-positive-one-direction",
            input: "one direction ka naya album aaya hai",
            must_contain: vec!["one direction"],
            must_not_contain: vec!["1 direction"],
        },

        // ═══════════════════════════════════════════════════════════
        //  DATES
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "D1-full-date",
            input: "twenty ninth may two thousand twenty six ko meeting schedule karo",
            must_contain: vec!["29", "May", "2026", "meeting"],
            must_not_contain: vec!["twenty ninth"],
        },
        TestCase {
            id: "D2-ordinal-of",
            input: "first of january se new policy lagu hogi",
            must_contain: vec!["1st", "January", "policy"],
            must_not_contain: vec!["first of january"],
        },
        TestCase {
            id: "D3-month-year",
            input: "march twenty twenty five mein yeh project shuru hua tha",
            must_contain: vec!["March", "2025", "project"],
            must_not_contain: vec!["twenty twenty five"],
        },
        TestCase {
            id: "D4-relative-day-no-format",
            input: "agle tuesday ko milte hain office mein",
            must_contain: vec!["tuesday", "office"],
            must_not_contain: vec![],
        },

        // ═══════════════════════════════════════════════════════════
        //  TIMES
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "T1-english-time",
            input: "kal eight thirty AM pe meeting hai conference room mein",
            must_contain: vec!["8:30 AM", "conference room"],
            must_not_contain: vec!["eight thirty"],
        },
        TestCase {
            id: "T2-hindi-time",
            input: "saat baje aana office subah mein",
            must_contain: vec!["7 baje", "office"],
            must_not_contain: vec!["saat baje"],
        },
        TestCase {
            id: "T3-quarter-to",
            input: "quarter to five baje tak kaam khatam karo",
            must_contain: vec!["4:45", "kaam khatam"],
            must_not_contain: vec![],
        },

        // ═══════════════════════════════════════════════════════════
        //  PHONE NUMBERS
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "P1-ten-digit",
            input: "nine eight seven six five four three two one zero pe call karo urgent hai",
            must_contain: vec!["9876543210", "call karo"],
            must_not_contain: vec!["nine eight"],
        },
        TestCase {
            id: "P2-plus91",
            input: "plus ninety one nine eight nine one one one one five four eight pe WhatsApp karo",
            must_contain: vec!["+91", "98911", "WhatsApp"],
            must_not_contain: vec!["ninety one nine"],
        },
        TestCase {
            id: "P3-emergency-no-format",
            input: "call me at nine one one for emergency",
            must_contain: vec!["emergency"],
            must_not_contain: vec![],
        },

        // ═══════════════════════════════════════════════════════════
        //  CURRENCY
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "C1-rupees",
            input: "five hundred rupees dena hai electrician ko",
            must_contain: vec!["500"],
            must_not_contain: vec!["five hundred rupees"],
        },
        TestCase {
            id: "C2-dollars",
            input: "fifty dollars per month ka plan hai",
            must_contain: vec!["$50", "plan"],
            must_not_contain: vec!["fifty dollars"],
        },
        TestCase {
            id: "C3-hindi-currency",
            input: "paanch sau rupaye mein teen kg aam milte hain",
            must_contain: vec!["500"],
            must_not_contain: vec!["paanch sau"],
        },

        // ═══════════════════════════════════════════════════════════
        //  IDENTIFIERS
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "I1-underscore-file",
            input: "file ka naam hai config underscore prod dot yaml",
            must_contain: vec!["config_prod.yaml"],
            must_not_contain: vec!["underscore"],
        },
        TestCase {
            id: "I2-hashtag",
            input: "hash tag trending pe aa gaya yaar",
            must_contain: vec!["trending", "yaar"],
            must_not_contain: vec![],
        },

        // ═══════════════════════════════════════════════════════════
        //  MEGA MIX — multiple replacements per sentence
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "M1-email-date-time-currency",
            input: "anish at the rate gmail dot com ko nineteenth may two thousand twenty six ko eight thirty AM pe five hundred rupees transfer karo",
            must_contain: vec!["anish@gmail.com", "19", "May", "2026", "8:30 AM", "500"],
            must_not_contain: vec!["at the rate"],
        },
        TestCase {
            id: "M2-number-percent-phone",
            input: "saat crore ka budget hai aur fifteen percent profit margin chahiye nine eight nine one one one one five four eight pe details bhejo",
            must_contain: vec!["7 crore", "15%", "9891"],
            must_not_contain: vec!["saat crore", "fifteen percent"],
        },
        TestCase {
            id: "M3-url-number",
            input: "double u double u double u dot emiac dot com pe jaake thirty two billion users ka data check karo",
            must_contain: vec!["www.emiac.com", "32 billion"],
            must_not_contain: vec!["double u", "thirty two"],
        },
        TestCase {
            id: "M4-date-time-number",
            input: "kal first january twenty twenty seven ko saat baje meeting hai paanch lakh ka proposal discuss karenge",
            must_contain: vec!["1st", "January", "2027", "7 baje", "5 lakh"],
            must_not_contain: vec![],
        },
        TestCase {
            id: "M5-two-emails-date",
            input: "rahul dot kumar at the rate yahoo dot com aur anish at the rate gmail dot com dono ko mail karo twenty ninth may tak",
            must_contain: vec!["yahoo.com", "anish@gmail.com", "29"],
            must_not_contain: vec!["at the rate"],
        },
        TestCase {
            id: "M6-url-phone",
            input: "h t t p s colon slash slash github dot com slash emiac slash said pe jaake plus ninety one nine eight seven six five four three two one zero pe call karo agar koi issue ho",
            must_contain: vec!["https://", "github.com", "+91", "9876543210"],
            must_not_contain: vec!["colon slash slash"],
        },

        // ═══════════════════════════════════════════════════════════
        //  HINDI VARIANT SPELLINGS — the whole reason regex was killed
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "H1-chah-baje",
            input: "mail likhna hai chah baje ka",
            must_contain: vec!["6 baje"],
            must_not_contain: vec!["chah baje"],
        },
        TestCase {
            id: "H2-chheh-baje",
            input: "meeting chheh baje hai office mein",
            must_contain: vec!["6 baje"],
            must_not_contain: vec!["chheh baje"],
        },
        TestCase {
            id: "H3-cheh-baje",
            input: "cheh baje tak aa jaana",
            must_contain: vec!["6 baje"],
            must_not_contain: vec!["cheh baje"],
        },
        TestCase {
            id: "H4-paanch-sau",
            input: "paanch sau rupaye de do usko",
            must_contain: vec!["500"],
            must_not_contain: vec!["paanch sau"],
        },
        TestCase {
            id: "H5-bees-hazaar",
            input: "bees hazaar ka phone khareedna hai",
            must_contain: vec!["20"],
            must_not_contain: vec!["bees hazaar"],
        },
        TestCase {
            id: "H6-email-plus-hindi-time",
            input: "Anish Suman two three zero five at the rate Gmail dot com ko mail likhna hai chah baje ka.",
            must_contain: vec!["@gmail.com", "6 baje"],
            must_not_contain: vec!["at the rate", "chah baje"],
        },
        TestCase {
            id: "H7-gyaarah-baje",
            input: "gyaarah baje doctor ke paas jaana hai",
            must_contain: vec!["11 baje"],
            must_not_contain: vec!["gyaarah baje"],
        },
        TestCase {
            id: "H8-dedh-sau",
            input: "dedh sau log aaye the function mein",
            must_contain: vec!["150"],
            must_not_contain: vec!["dedh sau"],
        },

        // ═══════════════════════════════════════════════════════════
        //  NO-OP — LLM should return replace: false
        // ═══════════════════════════════════════════════════════════
        TestCase {
            id: "NOP1-plain-english",
            input: "I finished the report and sent it to the team.",
            must_contain: vec!["finished the report"],
            must_not_contain: vec![],
        },
        TestCase {
            id: "NOP2-plain-hinglish",
            input: "Maine kaam kar liya hai aur ghar ja raha hoon.",
            must_contain: vec!["kaam kar liya"],
            must_not_contain: vec![],
        },
        TestCase {
            id: "NOP3-already-formatted",
            input: "Send it to anish@gmail.com by 6 PM.",
            must_contain: vec!["anish@gmail.com", "6 PM"],
            must_not_contain: vec![],
        },
        TestCase {
            id: "NOP4-brand-name-seven",
            input: "seven seas restaurant mein dinner karte hain aaj",
            must_contain: vec!["seven seas"],
            must_not_contain: vec!["7 seas"],
        },
    ]
}
