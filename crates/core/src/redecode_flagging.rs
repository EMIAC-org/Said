//! Confidence-gated re-decode span flagging for the meeting STT pipeline.
//!
//! After a first whisper pass (or the reused live transcript), most words are
//! highly confident (median token-prob ~0.97 on real Hinglish meetings). Only a
//! small tail is genuinely uncertain. Naively flagging whole segments — or even
//! every individual low-confidence word — re-decodes ~all of the audio, because
//! code-switched Hinglish sprinkles low-confidence English words and proper-noun
//! fragments evenly throughout.
//!
//! The signal that actually isolates the broken regions is **clustered** low
//! confidence: two or more low-confidence words close together, where the model
//! lost the thread. Isolated low-confidence words (a lone garble in otherwise
//! confident speech) are skipped — re-decoding a tiny isolated window loses
//! context and rarely improves.
//!
//! Empirically (12.7-min Hinglish meeting, 3,438 tokens), the defaults below
//! flag ~21% of audio at p<0.20, or ~4% at p<0.15 — versus 96% for naive
//! segment-level flagging. See the tests.

use serde::Deserialize;

/// A transcribed word/token with its time span and decoder confidence.
#[derive(Debug, Clone, Copy)]
pub struct WordConf {
    pub start_ms: u64,
    pub end_ms: u64,
    /// whisper.cpp per-token probability, 0.0..=1.0 (higher = more confident).
    pub prob: f32,
}

impl WordConf {
    fn mid_ms(&self) -> u64 {
        (self.start_ms + self.end_ms) / 2
    }
}

/// Tunables for cluster flagging. Defaults derived from a real Hinglish meeting.
#[derive(Debug, Clone, Copy)]
pub struct RedecodeConfig {
    /// A word counts as "low-confidence" below this token probability.
    pub prob_threshold: f32,
    /// Two low-confidence words within this gap form part of the same cluster.
    pub cluster_window_ms: u64,
    /// A re-decode span needs at least this many low-confidence words nearby.
    pub min_cluster_size: usize,
    /// Context padding added on each side of a flagged word.
    pub pad_ms: u64,
    /// Each re-decode window is at least this long (whisper needs context).
    pub min_span_ms: u64,
    /// Flagged spans closer than this are merged into one.
    pub merge_gap_ms: u64,
}

impl Default for RedecodeConfig {
    fn default() -> Self {
        // ~21% of audio on the validation meeting — the "catch real garbles,
        // skip isolated noise" operating point. Drop prob_threshold to 0.15 for
        // a ~4% conservative pass (worst garbles only).
        Self {
            prob_threshold: 0.20,
            cluster_window_ms: 4_000,
            min_cluster_size: 2,
            pad_ms: 1_000,
            min_span_ms: 3_000,
            merge_gap_ms: 2_000,
        }
    }
}

/// A time range worth re-decoding (with a bigger model / vocab biasing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedecodeSpan {
    pub start_ms: u64,
    pub end_ms: u64,
    /// How many low-confidence words fell inside this span.
    pub low_conf_words: usize,
}

impl RedecodeSpan {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// Flag the spans worth re-decoding: clusters of low-confidence words.
///
/// `words` must be in (or will be sorted into) time order. Returns disjoint,
/// time-ordered spans. Callers should clamp `end_ms` to the audio duration.
pub fn flag_redecode_spans(words: &[WordConf], cfg: &RedecodeConfig) -> Vec<RedecodeSpan> {
    // 1. midpoints of low-confidence words, in time order.
    let mut low: Vec<u64> = words
        .iter()
        .filter(|w| w.prob < cfg.prob_threshold)
        .map(WordConf::mid_ms)
        .collect();
    low.sort_unstable();
    if low.is_empty() {
        return Vec::new();
    }

    // 2. density gate: keep a word only if it has >= (min_cluster_size - 1)
    //    other low-confidence words within cluster_window_ms.
    let need_neighbours = cfg.min_cluster_size.saturating_sub(1);
    let kept: Vec<u64> = (0..low.len())
        .filter(|&i| {
            let c = low[i];
            let neighbours = low
                .iter()
                .enumerate()
                .filter(|&(j, &o)| {
                    j != i && (o as i64 - c as i64).unsigned_abs() <= cfg.cluster_window_ms
                })
                .count();
            neighbours >= need_neighbours
        })
        .map(|i| low[i])
        .collect();
    if kept.is_empty() {
        return Vec::new();
    }

    // 3. expand each kept word to a padded min-width window, merging neighbours.
    let half = cfg.min_span_ms / 2;
    let mut spans: Vec<RedecodeSpan> = Vec::new();
    for &c in &kept {
        let start = c.saturating_sub(half + cfg.pad_ms);
        let end = c + half + cfg.pad_ms;
        match spans.last_mut() {
            Some(last) if start <= last.end_ms + cfg.merge_gap_ms => {
                last.end_ms = last.end_ms.max(end);
                last.low_conf_words += 1;
            }
            _ => spans.push(RedecodeSpan {
                start_ms: start,
                end_ms: end,
                low_conf_words: 1,
            }),
        }
    }
    spans
}

/// Total flagged duration across spans (ms).
pub fn flagged_duration_ms(spans: &[RedecodeSpan]) -> u64 {
    spans.iter().map(RedecodeSpan::duration_ms).sum()
}

// ── Segment-level flagging ──────────────────────────────────────────────────
// Re-decoding whole segments (vs sub-spans) keeps clean boundaries and lets the
// caller preserve speaker labels/timestamps. We only flag a segment when it
// contains a *dense* low-confidence cluster, so most confident segments are
// left untouched. On a real Hinglish meeting this flags ~5/24 segments (~20% of
// audio) at prob_threshold 0.15 — vs 58% at 0.20, so 0.15 is the safe default
// for the whole-segment strategy.

/// One transcript segment with its words/confidences (from `-ojf` JSON).
#[derive(Debug, Clone)]
pub struct ConfSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub words: Vec<WordConf>,
}

/// Whether a segment contains a dense low-confidence cluster (>= min_cluster_size
/// low-confidence words within cluster_window_ms of each other).
pub fn segment_has_dense_low_conf(seg: &ConfSegment, cfg: &RedecodeConfig) -> bool {
    let low: Vec<u64> = seg
        .words
        .iter()
        .filter(|w| w.prob < cfg.prob_threshold)
        .map(WordConf::mid_ms)
        .collect();
    if low.len() < cfg.min_cluster_size {
        return false;
    }
    let need = cfg.min_cluster_size.saturating_sub(1);
    low.iter().any(|&c| {
        let neighbours = low
            .iter()
            .filter(|&&o| o != c && (o as i64 - c as i64).unsigned_abs() <= cfg.cluster_window_ms)
            .count();
        neighbours >= need
    })
}

/// Indices of segments worth re-decoding (those with a dense low-conf cluster).
pub fn flag_low_conf_segments(segs: &[ConfSegment], cfg: &RedecodeConfig) -> Vec<usize> {
    segs.iter()
        .enumerate()
        .filter(|(_, s)| segment_has_dense_low_conf(s, cfg))
        .map(|(i, _)| i)
        .collect()
}

/// Parse whisper.cpp full-JSON (`-ojf`) into per-segment word confidences,
/// preserving segment grouping + absolute time offsets. Returns empty on
/// malformed input.
pub fn conf_segments_from_whisper_json_full(json: &str) -> Vec<ConfSegment> {
    let out: WjOutput = match serde_json::from_str(json) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let mut segs = Vec::new();
    for seg in out.transcription {
        let Some(off) = seg.offsets else { continue };
        let mut words = Vec::new();
        for t in seg.tokens.unwrap_or_default() {
            if is_special(&t.text) {
                continue;
            }
            let (Some(p), Some(toff)) = (t.p, t.offsets) else {
                continue;
            };
            words.push(WordConf {
                start_ms: toff.from,
                end_ms: toff.to.max(toff.from),
                prob: p as f32,
            });
        }
        if words.is_empty() {
            continue;
        }
        segs.push(ConfSegment {
            start_ms: off.from,
            end_ms: off.to.max(off.from),
            words,
        });
    }
    segs
}

// ── whisper.cpp `-ojf` JSON → WordConf ──────────────────────────────────────
// The meeting pipeline already parses segment text + token `p`, but not the
// per-token `offsets`. This adds them so spans can be located in time.

#[derive(Deserialize)]
struct WjOutput {
    transcription: Vec<WjSegment>,
}
#[derive(Deserialize)]
struct WjSegment {
    offsets: Option<WjOffsets>,
    tokens: Option<Vec<WjToken>>,
}
#[derive(Deserialize)]
struct WjToken {
    text: String,
    p: Option<f64>,
    offsets: Option<WjOffsets>,
}
#[derive(Deserialize)]
struct WjOffsets {
    from: u64,
    to: u64,
}

fn is_special(text: &str) -> bool {
    let t = text.trim();
    t.is_empty() || t.starts_with("[_") || (t.starts_with("<|") && t.ends_with("|>"))
}

/// Parse whisper.cpp full-JSON (`-ojf`) into per-word confidences, dropping
/// special/timestamp tokens. Returns empty on malformed input.
pub fn words_from_whisper_json_full(json: &str) -> Vec<WordConf> {
    let out: WjOutput = match serde_json::from_str(json) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let mut words = Vec::new();
    for seg in out.transcription {
        for t in seg.tokens.unwrap_or_default() {
            if is_special(&t.text) {
                continue;
            }
            let (Some(p), Some(off)) = (t.p, t.offsets) else {
                continue;
            };
            words.push(WordConf {
                start_ms: off.from,
                end_ms: off.to.max(off.from),
                prob: p as f32,
            });
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: u64, end: u64, p: f32) -> WordConf {
        WordConf {
            start_ms: start,
            end_ms: end,
            prob: p,
        }
    }

    #[test]
    fn isolated_low_conf_word_is_not_flagged() {
        // one bad word in a sea of confident ones → skip (re-decoding it in
        // isolation would lose context and rarely help).
        let words = vec![
            w(0, 500, 0.98),
            w(500, 1000, 0.99),
            w(1000, 1500, 0.05), // lone garble
            w(1500, 2000, 0.97),
            w(2000, 2500, 0.99),
        ];
        let spans = flag_redecode_spans(&words, &RedecodeConfig::default());
        assert!(
            spans.is_empty(),
            "isolated low-conf word must not flag a span"
        );
    }

    #[test]
    fn clustered_low_conf_words_are_flagged_once() {
        // two bad words close together → the model lost the thread → flag, and
        // they merge into a single span.
        let words = vec![
            w(0, 500, 0.98),
            w(3000, 3400, 0.08),
            w(3500, 3900, 0.12), // within 4s of the previous → cluster
            w(8000, 8500, 0.99),
        ];
        let spans = flag_redecode_spans(&words, &RedecodeConfig::default());
        assert_eq!(spans.len(), 1, "one cluster → one merged span");
        assert_eq!(spans[0].low_conf_words, 2);
        assert!(spans[0].duration_ms() >= RedecodeConfig::default().min_span_ms);
    }

    #[test]
    fn empty_when_all_confident() {
        let words = vec![w(0, 500, 0.95), w(500, 1000, 0.99)];
        assert!(flag_redecode_spans(&words, &RedecodeConfig::default()).is_empty());
    }

    #[test]
    fn segment_flagging_needs_a_cluster() {
        let cfg = RedecodeConfig::default();
        // confident segment → not flagged
        let good = ConfSegment {
            start_ms: 0,
            end_ms: 30_000,
            words: vec![w(0, 500, 0.95), w(600, 1100, 0.99), w(1200, 1700, 0.9)],
        };
        assert!(!segment_has_dense_low_conf(&good, &cfg));
        // one isolated low-conf word → not flagged
        let isolated = ConfSegment {
            start_ms: 0,
            end_ms: 30_000,
            words: vec![w(0, 500, 0.95), w(600, 1100, 0.05), w(20_000, 20_500, 0.99)],
        };
        assert!(!segment_has_dense_low_conf(&isolated, &cfg));
        // two low-conf words close together → flagged
        let bad = ConfSegment {
            start_ms: 0,
            end_ms: 30_000,
            words: vec![w(0, 500, 0.95), w(3000, 3400, 0.08), w(3500, 3900, 0.10)],
        };
        assert!(segment_has_dense_low_conf(&bad, &cfg));
        assert_eq!(
            flag_low_conf_segments(&[good, isolated, bad], &cfg),
            vec![2]
        );
    }

    #[test]
    fn real_meeting_segment_flagging_is_small() {
        let path = "/tmp/meeting_conf.json";
        let Ok(json) = std::fs::read_to_string(path) else {
            eprintln!("[skip] fixture {path} not present");
            return;
        };
        let segs = conf_segments_from_whisper_json_full(&json);
        assert!(!segs.is_empty());
        let cfg = RedecodeConfig {
            prob_threshold: 0.15,
            ..Default::default()
        };
        let flagged = flag_low_conf_segments(&segs, &cfg);
        let flagged_ms: u64 = flagged
            .iter()
            .map(|&i| segs[i].end_ms - segs[i].start_ms)
            .sum();
        let total_ms: u64 = segs.iter().map(|s| s.end_ms - s.start_ms).sum();
        let pct = flagged_ms * 100 / total_ms.max(1);
        eprintln!(
            "segment-level @0.15: {}/{} segments flagged, {}% of audio",
            flagged.len(),
            segs.len(),
            pct
        );
        assert!(
            pct < 35,
            "whole-segment re-decode should stay small, got {pct}%"
        );
    }

    /// Validation against the real meeting JSON captured during the live test.
    /// Skips cleanly when the fixture isn't present (CI / other machines).
    #[test]
    fn real_meeting_flags_small_fraction() {
        let path = "/tmp/meeting_conf.json";
        let Ok(json) = std::fs::read_to_string(path) else {
            eprintln!("[skip] fixture {path} not present");
            return;
        };
        let words = words_from_whisper_json_full(&json);
        assert!(!words.is_empty(), "fixture parsed to zero words");
        let span_total = words.last().unwrap().end_ms - words.first().unwrap().start_ms;

        for thr in [0.15_f32, 0.20] {
            let cfg = RedecodeConfig {
                prob_threshold: thr,
                ..Default::default()
            };
            let spans = flag_redecode_spans(&words, &cfg);
            let cov = flagged_duration_ms(&spans);
            let pct = cov * 100 / span_total.max(1);
            let low = words.iter().filter(|x| x.prob < thr).count();
            eprintln!(
                "thr={thr:.2}: {} words, {low} low-conf, {} spans, {}s / {}s re-decoded ({pct}%)",
                words.len(),
                spans.len(),
                cov / 1000,
                span_total / 1000,
            );
            assert!(
                pct < 35,
                "thr={thr}: expected to re-decode <35% of audio, got {pct}%"
            );
        }
    }
}
