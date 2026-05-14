//! Exhaustive Devanagari → Roman transliteration benchmark.
//!
//! Tests coverage, correctness, quality, and latency of the current
//! romanizer across the full Unicode Devanagari block + real-world
//! Hinglish sentences. Outputs an HTML report to stdout.
//!
//! Run:
//!   cargo test -p said-backend --test romanizer_bench -- --nocapture > romanizer-report.html

use said_backend::llm::script;
use std::time::Instant;

// ── Full Devanagari Unicode block (ऀ–ॿ) ─────────────────────────
const DEVANAGARI_RANGE: std::ops::RangeInclusive<u32> = 0x0900..=0x097F;

fn all_devanagari_chars() -> Vec<char> {
    DEVANAGARI_RANGE
        .filter_map(char::from_u32)
        .filter(|c| !c.is_control())
        .collect()
}

// ── Test corpus: real-world Devanagari sentences ──────────────────────────
const HINDI_SENTENCES: &[(&str, &str)] = &[
    ("नमस्ते, आप कैसे हैं?", "Namaste, aap kaise hain?"),
    ("मैं ठीक हूँ, धन्यवाद।", "Main theek hoon, dhanyvaad."),
    ("यह बहुत अच्छा है।", "Yeh bahut achha hai."),
    ("कृपया मुझे बताइए।", "Kripya mujhe bataie."),
    ("भारत एक विशाल देश है।", "Bhaarat ek vishaal desh hai."),
    ("मुझे हिंदी बोलना पसंद है।", "Mujhe Hindi bolna pasand hai."),
    ("आज मौसम बहुत सुहाना है।", "Aaj mausam bahut suhaana hai."),
    ("क्या आप कॉफ़ी पिएंगे?", "Kya aap coffee pienge?"),
    ("मेरा नाम अभिषेक है।", "Mera naam Abhishek hai."),
    (
        "दिल्ली भारत की राजधानी है।",
        "Dilli Bhaarat ki raajdhaani hai.",
    ),
    ("खुश रहो, मस्त रहो।", "Khush raho, mast raho."),
    ("कंप्यूटर विज्ञान पढ़ रहा हूँ।", "Computer vigyaan padh raha hoon."),
    ("ज़िंदगी बहुत ख़ूबसूरत है।", "Zindagi bahut khoobsoorat hai."),
    ("घर जाना है, बहुत थक गया।", "Ghar jaana hai, bahut thak gaya."),
    ("पानी पीना ज़रूरी है।", "Paani peena zaroori hai."),
];

// ── Mixed Hinglish (what the LLM actually outputs) ───────────────────────
const MIXED_HINGLISH: &[&str] = &[
    "Bhai, iska IPO कब aayega? Mujhe यह bata do jaldi se.",
    "Meeting में kya hua tha? Please बताओ details.",
    "I think यह काम हो jayega by tomorrow evening.",
    "Server को restart karo, bahut धीमा chal raha hai.",
    "उसने कहा ki woh nahi aayega, so we should plan without him.",
    "Main office जा raha hoon, तुम bhi aao if possible.",
    "Deepgram का API key expire हो gaya, naya generate karo.",
    "Recording भेजना meeting का। Emiac Technologies की meeting में.",
    "अभी तो काम चल रहा है but we need more resources.",
    "कृपया ध्यान दें: this is very important for the project.",
    "Next sprint में हमें यह feature ship करना है definitely.",
    "उसका phone नहीं लग raha, शायद busy होगा.",
    "ठीक है, तो हम कल discuss करेंगे about the timeline.",
    "बहुत बढ़िया! The deployment went smoothly without any issues.",
    "इसको देखना अच्छे से कि main kaise isko aage kaam mein laga sakta hoon.",
];

// ── Stress test: long Devanagari paragraphs ──────────────────────────────
const LONG_PARAGRAPH: &str = "\
भारत, आधिकारिक तौर पर भारत गणराज्य, दक्षिण एशिया में स्थित एक देश है। \
यह क्षेत्रफल के अनुसार सातवाँ सबसे बड़ा देश है और जनसंख्या के अनुसार दूसरा सबसे बड़ा देश है। \
भारत का संविधान 26 जनवरी 1950 को लागू हुआ था, जिसे गणतंत्र दिवस के रूप में मनाया जाता है। \
हिंदी और अंग्रेज़ी भारत की दो आधिकारिक भाषाएँ हैं। भारत में कुल 22 अनुसूचित भाषाएँ हैं। \
भारत की अर्थव्यवस्था विश्व की पाँचवीं सबसे बड़ी अर्थव्यवस्था है। सॉफ़्टवेयर निर्यात, \
कृषि, विनिर्माण और सेवा क्षेत्र भारतीय अर्थव्यवस्था के प्रमुख स्तंभ हैं। भारतीय अंतरिक्ष \
अनुसंधान संगठन ने कई सफल अभियान चलाए हैं। चंद्रयान, मंगलयान और गगनयान भारत की \
प्रमुख अंतरिक्ष परियोजनाएँ हैं। प्रधानमंत्री कार्यालय नई दिल्ली में स्थित है। \
भारत के राष्ट्रपति राष्ट्रपति भवन में निवास करते हैं।";

const EDGE_CASES: &[(&str, &str)] = &[
    // Conjuncts
    ("क्ष", "ksh"),
    ("त्र", "tr"),
    ("ज्ञ", "gny"),
    ("श्र", "shr"),
    // Nukta variants
    ("क़", "q"),
    ("ख़", "kh"),
    ("ग़", "gh"),
    ("ज़", "z"),
    ("फ़", "f"),
    ("ड़", "d"),
    ("ढ़", "dh"),
    // Vowel signs
    ("कि", "ki"),
    ("की", "kee"),
    ("कु", "ku"),
    ("कू", "koo"),
    ("कृ", "kri"),
    ("के", "ke"),
    ("कै", "kai"),
    ("को", "ko"),
    ("कौ", "kau"),
    ("का", "kaa"),
    // Nasals
    ("अं", "an"),
    ("अँ", "an"),
    ("अः", "ah"),
    // Halant
    ("क्", "k"),
    // Independent vowels
    ("अ", "a"),
    ("आ", "aa"),
    ("इ", "i"),
    ("ई", "ee"),
    ("उ", "u"),
    ("ऊ", "oo"),
    ("ऋ", "ri"),
    ("ए", "e"),
    ("ऐ", "ai"),
    ("ओ", "o"),
    ("औ", "au"),
    // Common words
    ("नमस्ते", "namaste"),
    ("धन्यवाद", "dhanyvaad"),
    ("भारत", "Bhaarat"),
    ("हिंदी", "Hindi"),
    ("कंप्यूटर", "computer"),
    // Virama + consonant clusters
    ("स्थान", "sthaan"),
    ("प्रधान", "pradhaan"),
    ("विद्यालय", "vidyaalay"),
];

// ── Benchmark runner ─────────────────────────────────────────────────────

struct BenchResult {
    label: String,
    input_chars: usize,
    output_chars: usize,
    devanagari_remaining: usize,
    latency_us: u64,
    iterations: usize,
}

fn count_devanagari(text: &str) -> usize {
    text.chars()
        .filter(|c| ('\u{0900}'..='\u{097F}').contains(c))
        .count()
}

fn regex_strip_devanagari(text: &str) -> String {
    text.chars()
        .filter(|c| !('\u{0900}'..='\u{097F}').contains(c))
        .collect()
}

fn romanize_then_strip(text: &str) -> String {
    let romanized = script::enforce_roman_hinglish(text);
    regex_strip_devanagari(&romanized)
}

fn bench_fn<F: Fn(&str) -> String>(
    label: &str,
    input: &str,
    f: F,
    iterations: usize,
) -> BenchResult {
    // Warm up
    for _ in 0..10 {
        let _ = f(input);
    }
    let start = Instant::now();
    let mut output = String::new();
    for _ in 0..iterations {
        output = f(input);
    }
    let elapsed = start.elapsed();
    BenchResult {
        label: label.to_string(),
        input_chars: input.chars().count(),
        output_chars: output.chars().count(),
        devanagari_remaining: count_devanagari(&output),
        latency_us: elapsed.as_micros() as u64 / iterations as u64,
        iterations,
    }
}

// ── Coverage test ────────────────────────────────────────────────────────

struct CoverageResult {
    total_chars: usize,
    handled_chars: usize,
    leaked_chars: Vec<(char, u32)>,
}

fn test_unicode_coverage() -> CoverageResult {
    let all = all_devanagari_chars();
    let total = all.len();
    let mut leaked = Vec::new();

    for ch in &all {
        let input = ch.to_string();
        let output = script::enforce_roman_hinglish(&input);
        if count_devanagari(&output) > 0 {
            leaked.push((*ch, *ch as u32));
        }
    }

    CoverageResult {
        total_chars: total,
        handled_chars: total - leaked.len(),
        leaked_chars: leaked,
    }
}

// ── Correctness test ─────────────────────────────────────────────────────

struct CorrectnessResult {
    test_name: String,
    input: String,
    expected_hint: String,
    actual: String,
    devanagari_free: bool,
    visually_close: bool,
}

fn test_correctness() -> Vec<CorrectnessResult> {
    let mut results = Vec::new();

    for (input, expected) in EDGE_CASES {
        let actual = script::enforce_roman_hinglish(input);
        let dev_free = count_devanagari(&actual) == 0;
        let close = actual.to_lowercase() == expected.to_lowercase();
        results.push(CorrectnessResult {
            test_name: format!("edge: {input}"),
            input: input.to_string(),
            expected_hint: expected.to_string(),
            actual,
            devanagari_free: dev_free,
            visually_close: close,
        });
    }

    for (input, expected) in HINDI_SENTENCES {
        let actual = script::enforce_roman_hinglish(input);
        let dev_free = count_devanagari(&actual) == 0;
        results.push(CorrectnessResult {
            test_name: format!("sentence: {}…", &input.chars().take(20).collect::<String>()),
            input: input.to_string(),
            expected_hint: expected.to_string(),
            actual: actual.clone(),
            devanagari_free: dev_free,
            visually_close: false, // sentences are approximate
        });
    }

    results
}

// ── Mixed Hinglish test ──────────────────────────────────────────────────

struct MixedResult {
    input: String,
    output_romanize: String,
    output_romanize_strip: String,
    output_strip_only: String,
    dev_remaining_romanize: usize,
    dev_remaining_strip: usize,
}

fn test_mixed() -> Vec<MixedResult> {
    MIXED_HINGLISH
        .iter()
        .map(|input| {
            let romanized = script::enforce_roman_hinglish(input);
            let romanize_strip = romanize_then_strip(input);
            let strip_only = regex_strip_devanagari(input);
            MixedResult {
                input: input.to_string(),
                dev_remaining_romanize: count_devanagari(&romanized),
                dev_remaining_strip: count_devanagari(&romanize_strip),
                output_romanize: romanized,
                output_romanize_strip: romanize_strip,
                output_strip_only: strip_only,
            }
        })
        .collect()
}

// ── HTML report generator ────────────────────────────────────────────────

fn generate_html(
    coverage: &CoverageResult,
    correctness: &[CorrectnessResult],
    mixed: &[MixedResult],
    benches: &[BenchResult],
) -> String {
    let mut html = String::new();

    html.push_str(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>Said Romanizer — Benchmark Report</title>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: #0d1117; color: #c9d1d9; padding: 40px; line-height: 1.6; }
h1 { font-size: 28px; margin-bottom: 8px; color: #f0f6fc; }
h2 { font-size: 18px; margin: 40px 0 16px; color: #f0f6fc; border-bottom: 1px solid #21262d; padding-bottom: 8px; }
h3 { font-size: 14px; margin: 20px 0 8px; color: #8b949e; text-transform: uppercase; letter-spacing: 0.05em; }
.subtitle { color: #8b949e; font-size: 14px; margin-bottom: 32px; }
table { width: 100%; border-collapse: collapse; margin: 12px 0 24px; font-size: 13px; }
th { text-align: left; padding: 8px 12px; background: #161b22; color: #8b949e; font-weight: 600; border-bottom: 1px solid #21262d; }
td { padding: 8px 12px; border-bottom: 1px solid #21262d; vertical-align: top; }
tr:hover td { background: #161b22; }
.pass { color: #3fb950; font-weight: 600; }
.fail { color: #f85149; font-weight: 600; }
.warn { color: #d29922; font-weight: 600; }
.mono { font-family: 'SF Mono', Menlo, monospace; font-size: 12px; }
.chip { display: inline-block; padding: 2px 8px; border-radius: 12px; font-size: 11px; font-weight: 600; }
.chip-green { background: rgba(63,185,80,0.15); color: #3fb950; }
.chip-red { background: rgba(248,81,73,0.15); color: #f85149; }
.chip-yellow { background: rgba(210,153,34,0.15); color: #d29922; }
.summary-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 16px; margin: 16px 0 32px; }
.summary-card { background: #161b22; border: 1px solid #21262d; border-radius: 12px; padding: 20px; }
.summary-card .value { font-size: 32px; font-weight: 700; color: #f0f6fc; }
.summary-card .label { font-size: 12px; color: #8b949e; margin-top: 4px; }
.bar { height: 8px; border-radius: 4px; background: #21262d; overflow: hidden; margin-top: 8px; }
.bar-fill { height: 100%; border-radius: 4px; }
pre { background: #161b22; border: 1px solid #21262d; border-radius: 8px; padding: 12px 16px; overflow-x: auto; font-size: 12px; margin: 8px 0; }
.leaked-char { display: inline-block; padding: 4px 8px; margin: 2px; background: rgba(248,81,73,0.1); border: 1px solid rgba(248,81,73,0.3); border-radius: 6px; font-size: 13px; }
</style>
</head>
<body>
<h1>🔤 Said Romanizer — Benchmark Report</h1>
<p class="subtitle">Devanagari → Roman transliteration coverage, correctness, and latency analysis</p>
"#);

    // ── Summary cards ─────────────────────────────────────────────────
    let coverage_pct = (coverage.handled_chars as f64 / coverage.total_chars as f64 * 100.0) as u32;
    let correctness_pass = correctness.iter().filter(|c| c.devanagari_free).count();
    let correctness_total = correctness.len();
    let mixed_clean = mixed
        .iter()
        .filter(|m| m.dev_remaining_romanize == 0)
        .count();
    let mixed_total = mixed.len();
    let avg_latency = if benches.is_empty() {
        0
    } else {
        benches.iter().map(|b| b.latency_us).sum::<u64>() / benches.len() as u64
    };

    html.push_str(&format!(r#"
<div class="summary-grid">
  <div class="summary-card">
    <div class="value" style="color: {}">{coverage_pct}%</div>
    <div class="label">Unicode Coverage ({}/{} chars)</div>
    <div class="bar"><div class="bar-fill" style="width: {coverage_pct}%; background: {};"></div></div>
  </div>
  <div class="summary-card">
    <div class="value" style="color: {}">{correctness_pass}/{correctness_total}</div>
    <div class="label">Devanagari-Free Outputs</div>
    <div class="bar"><div class="bar-fill" style="width: {}%; background: {};"></div></div>
  </div>
  <div class="summary-card">
    <div class="value" style="color: {}">{mixed_clean}/{mixed_total}</div>
    <div class="label">Mixed Hinglish Clean</div>
    <div class="bar"><div class="bar-fill" style="width: {}%; background: {};"></div></div>
  </div>
  <div class="summary-card">
    <div class="value">{avg_latency}μs</div>
    <div class="label">Avg Latency per Call</div>
  </div>
</div>"#,
        if coverage_pct >= 95 { "#3fb950" } else { "#f85149" },
        coverage.handled_chars, coverage.total_chars,
        if coverage_pct >= 95 { "#3fb950" } else { "#f85149" },
        if correctness_pass == correctness_total { "#3fb950" } else { "#f85149" },
        (correctness_pass * 100 / correctness_total),
        if correctness_pass == correctness_total { "#3fb950" } else { "#f85149" },
        if mixed_clean == mixed_total { "#3fb950" } else { "#d29922" },
        (mixed_clean * 100 / mixed_total),
        if mixed_clean == mixed_total { "#3fb950" } else { "#d29922" },
    ));

    // ── Unicode coverage ──────────────────────────────────────────────
    html.push_str("<h2>1. Unicode Coverage (\\u{0900}–\\u{097F})</h2>");
    if coverage.leaked_chars.is_empty() {
        html.push_str(r#"<p><span class="chip chip-green">✓ FULL COVERAGE</span> All Devanagari characters are handled.</p>"#);
    } else {
        html.push_str(&format!(
            r#"<p><span class="chip chip-red">✗ {} LEAKED</span> These characters pass through the romanizer unchanged:</p><div style="margin: 12px 0;">"#,
            coverage.leaked_chars.len()
        ));
        for (ch, code) in &coverage.leaked_chars {
            html.push_str(&format!(
                r#"<span class="leaked-char"><span class="mono">U+{code:04X}</span> {ch}</span>"#
            ));
        }
        html.push_str("</div>");
    }

    // ── Correctness ───────────────────────────────────────────────────
    html.push_str("<h2>2. Correctness — Edge Cases & Sentences</h2>");
    html.push_str(r#"<table>
<tr><th>Test</th><th>Input</th><th>Expected (hint)</th><th>Actual</th><th>Dev-Free</th><th>Match</th></tr>"#);
    for c in correctness {
        let dev_badge = if c.devanagari_free {
            r#"<span class="pass">✓</span>"#
        } else {
            r#"<span class="fail">✗</span>"#
        };
        let match_badge = if c.visually_close {
            r#"<span class="pass">✓</span>"#
        } else if c.test_name.starts_with("sentence") {
            "—"
        } else {
            r#"<span class="warn">≈</span>"#
        };
        html.push_str(&format!(
            r#"<tr><td>{}</td><td class="mono">{}</td><td class="mono">{}</td><td class="mono">{}</td><td>{dev_badge}</td><td>{match_badge}</td></tr>"#,
            esc(&c.test_name), esc(&c.input), esc(&c.expected_hint), esc(&c.actual),
        ));
    }
    html.push_str("</table>");

    // ── Mixed Hinglish ────────────────────────────────────────────────
    html.push_str("<h2>3. Mixed Hinglish — Real LLM Output Simulation</h2>");
    html.push_str("<h3>Approach comparison: romanize vs romanize+strip vs strip-only</h3>");
    html.push_str(r#"<table>
<tr><th style="width:25%">Input</th><th style="width:25%">romanize()</th><th>romanize+strip</th><th>Dev left (romanize)</th><th>Dev left (r+s)</th></tr>"#);
    for m in mixed {
        let badge1 = if m.dev_remaining_romanize == 0 {
            r#"<span class="pass">0</span>"#
        } else {
            &format!(r#"<span class="fail">{}</span>"#, m.dev_remaining_romanize)
        };
        let badge2 = if m.dev_remaining_strip == 0 {
            r#"<span class="pass">0</span>"#
        } else {
            &format!(r#"<span class="fail">{}</span>"#, m.dev_remaining_strip)
        };
        html.push_str(&format!(
            r#"<tr><td class="mono" style="font-size:11px">{}</td><td class="mono" style="font-size:11px">{}</td><td class="mono" style="font-size:11px">{}</td><td>{badge1}</td><td>{badge2}</td></tr>"#,
            esc(&m.input), esc(&m.output_romanize), esc(&m.output_romanize_strip),
        ));
    }
    html.push_str("</table>");

    // ── Latency benchmarks ────────────────────────────────────────────
    html.push_str("<h2>4. Latency Benchmarks</h2>");
    html.push_str(r#"<table>
<tr><th>Approach</th><th>Input Size</th><th>Output Size</th><th>Dev Remaining</th><th>Latency (μs)</th><th>Iterations</th></tr>"#);
    for b in benches {
        let dev_badge = if b.devanagari_remaining == 0 {
            r#"<span class="pass">0</span>"#.to_string()
        } else {
            format!(r#"<span class="fail">{}</span>"#, b.devanagari_remaining)
        };
        html.push_str(&format!(
            r#"<tr><td>{}</td><td>{} chars</td><td>{} chars</td><td>{dev_badge}</td><td><strong>{}</strong> μs</td><td>{}</td></tr>"#,
            esc(&b.label), b.input_chars, b.output_chars, b.latency_us, b.iterations,
        ));
    }
    html.push_str("</table>");

    // ── Recommendation ────────────────────────────────────────────────
    let has_leaks = !coverage.leaked_chars.is_empty()
        || correctness.iter().any(|c| !c.devanagari_free)
        || mixed.iter().any(|m| m.dev_remaining_romanize > 0);

    html.push_str("<h2>5. Recommendation</h2>");
    if has_leaks {
        html.push_str(r#"<div style="background: rgba(248,81,73,0.08); border: 1px solid rgba(248,81,73,0.3); border-radius: 12px; padding: 20px; margin: 16px 0;">
<p style="font-weight: 600; color: #f85149; margin-bottom: 8px;">⚠ Devanagari leakage detected</p>
<p>The current <code>enforce_roman_hinglish()</code> does not cover all Unicode Devanagari characters. Recommended fix:</p>
<ol style="margin: 12px 0 0 20px; line-height: 2;">
<li><strong>Expand character tables</strong> in <code>script.rs</code> to cover all leaked characters, OR replace with <code>vidyut-lipi</code> crate</li>
<li><strong>Add regex safety net</strong> as the absolute last step: strip any surviving <code>\u{0900}–\u{097F}</code></li>
<li><strong>Move the scrub after <code>format_recover</code></strong> in voice.rs (currently runs before)</li>
<li><strong>Add desktop-side check</strong> before pasting</li>
</ol>
</div>"#);
    } else {
        html.push_str(r#"<div style="background: rgba(63,185,80,0.08); border: 1px solid rgba(63,185,80,0.3); border-radius: 12px; padding: 20px;">
<p style="font-weight: 600; color: #3fb950;">✓ Full coverage — no Devanagari leakage detected</p>
<p>The romanizer handles all tested Unicode Devanagari characters. The regex safety net would add defense-in-depth.</p>
</div>"#);
    }

    html.push_str(r#"
<p style="margin-top: 40px; font-size: 11px; color: #484f58;">Generated by Said romanizer-bench · said-backend</p>
</body>
</html>"#);

    html
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ── Main test ────────────────────────────────────────────────────────────

#[test]
fn romanizer_benchmark_report() {
    eprintln!("Running romanizer benchmark...");

    // 1. Unicode coverage
    eprintln!("  [1/4] Unicode coverage test...");
    let coverage = test_unicode_coverage();
    eprintln!(
        "    Coverage: {}/{} ({} leaked)",
        coverage.handled_chars,
        coverage.total_chars,
        coverage.leaked_chars.len()
    );

    // 2. Correctness
    eprintln!("  [2/4] Correctness tests...");
    let correctness = test_correctness();
    let dev_free = correctness.iter().filter(|c| c.devanagari_free).count();
    eprintln!("    Devanagari-free: {}/{}", dev_free, correctness.len());

    // 3. Mixed Hinglish
    eprintln!("  [3/4] Mixed Hinglish tests...");
    let mixed = test_mixed();
    let clean = mixed
        .iter()
        .filter(|m| m.dev_remaining_romanize == 0)
        .count();
    eprintln!("    Clean outputs: {}/{}", clean, mixed.len());

    // 4. Latency benchmarks
    eprintln!("  [4/4] Latency benchmarks...");
    let iterations = 10_000;
    let mut benches = Vec::new();

    // Short sentence
    let short = "नमस्ते, आप कैसे हैं?";
    benches.push(bench_fn(
        "romanize() — short",
        short,
        |t| script::enforce_roman_hinglish(t),
        iterations,
    ));
    benches.push(bench_fn(
        "romanize+strip — short",
        short,
        romanize_then_strip,
        iterations,
    ));
    benches.push(bench_fn(
        "strip only — short",
        short,
        |t| regex_strip_devanagari(t),
        iterations,
    ));

    // Long paragraph
    benches.push(bench_fn(
        "romanize() — long (430 chars)",
        LONG_PARAGRAPH,
        |t| script::enforce_roman_hinglish(t),
        iterations,
    ));
    benches.push(bench_fn(
        "romanize+strip — long",
        LONG_PARAGRAPH,
        romanize_then_strip,
        iterations,
    ));
    benches.push(bench_fn(
        "strip only — long",
        LONG_PARAGRAPH,
        |t| regex_strip_devanagari(t),
        iterations,
    ));

    // Mixed Hinglish
    let mixed_input = MIXED_HINGLISH.join(" ");
    benches.push(bench_fn(
        "romanize() — mixed hinglish",
        &mixed_input,
        |t| script::enforce_roman_hinglish(t),
        iterations,
    ));
    benches.push(bench_fn(
        "romanize+strip — mixed",
        &mixed_input,
        romanize_then_strip,
        iterations,
    ));

    // Mega stress: 10x long paragraph
    let mega = LONG_PARAGRAPH.repeat(10);
    benches.push(bench_fn(
        "romanize() — mega (4300 chars)",
        &mega,
        |t| script::enforce_roman_hinglish(t),
        1_000,
    ));
    benches.push(bench_fn(
        "romanize+strip — mega",
        &mega,
        romanize_then_strip,
        1_000,
    ));

    for b in &benches {
        eprintln!(
            "    {} → {}μs (dev_remaining={})",
            b.label, b.latency_us, b.devanagari_remaining
        );
    }

    // Generate HTML
    let html = generate_html(&coverage, &correctness, &mixed, &benches);
    println!("{html}");

    eprintln!("\n✓ Report written to stdout. Redirect to file:");
    eprintln!(
        "  cargo test -p said-backend --test romanizer_bench -- --nocapture > romanizer-report.html"
    );
}
