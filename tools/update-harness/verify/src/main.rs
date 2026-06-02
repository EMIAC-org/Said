//! Reproduces tauri-plugin-updater's `verify_signature`: base64-unwrap the
//! pubkey + signature (Tauri's format), decode as minisign, then verify the
//! bundle bytes. Proves the supply-chain gate: an untouched bundle verifies,
//! a single-byte-tampered bundle is rejected — exactly what `download()` does
//! before it will install anything.
//!
//! Args: <pubkey-b64-file> <sig-b64-file> <bundle-file>

use std::fs;

use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};

fn unwrap_b64(path: &str) -> String {
    let b64 = fs::read_to_string(path).expect("read file").trim().to_string();
    String::from_utf8(STANDARD.decode(b64).expect("base64 decode")).expect("utf8")
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 {
        eprintln!("usage: verify-sig <pubkey-b64-file> <sig-b64-file> <bundle-file>");
        std::process::exit(2);
    }
    let pk = PublicKey::decode(unwrap_b64(&a[1]).trim()).expect("decode pubkey");
    let sig = Signature::decode(&unwrap_b64(&a[2])).expect("decode signature");
    let data = fs::read(&a[3]).expect("read bundle");

    println!("verifying {} bytes against the TEST public key\n", data.len());

    match pk.verify(&data, &sig, true) {
        Ok(()) => println!("[untouched bundle]  ✅ VERIFY OK      → app would install"),
        Err(e) => println!("[untouched bundle]  ❌ VERIFY FAILED  → {e}"),
    }

    // Attacker swaps the binary but keeps the (now-mismatched) signature.
    let mut tampered = data.clone();
    if let Some(b) = tampered.first_mut() {
        *b ^= 0x01;
    }
    match pk.verify(&tampered, &sig, true) {
        Ok(()) => println!("[tampered  bundle]  ⚠️ VERIFY OK      → BAD: would have installed!"),
        Err(e) => println!("[tampered  bundle]  ✅ REJECTED        → {e}"),
    }
}
