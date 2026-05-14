#!/usr/bin/env -S cargo +nightly -Zscript
//! Romanizer benchmark — tests coverage, correctness, and latency of
//! Devanagari → Roman transliteration approaches. Outputs HTML report.
//!
//! Run:  cargo run --release -p said-backend --bin romanizer-bench
//! Or:   rustc scripts/romanizer-bench.rs ... (standalone won't work, uses crate internals)

// This file is meant to be compiled as a test binary inside said-backend.
// See the actual runner below.
