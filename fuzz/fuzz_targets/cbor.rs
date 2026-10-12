//! Fuzz the CBOR parser (§15).
//!
//! §9.2 makes CBOR the typical content type, so this parser runs on the body of almost
//! every symbol — and by the time it does, the bytes have survived ECC but nothing has
//! vouched for their *shape*. A CRC says the bits arrived as sent; it says nothing about
//! whether what was sent is well-formed, and an attacker controls what was sent.
//!
//! Two failure modes worth catching, neither of which a round-trip test would find,
//! because a round trip only ever feeds the parser what the encoder produced:
//!
//! * **Panics** — a slice index or an arithmetic overflow on a truncated or absurd header.
//! * **Unbounded recursion** — nested arrays or maps are the classic stack-exhaustion
//!   shape, and `a1` repeated a few thousand times is a very short input.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = lcr_core::cbor::Value::from_slice(data) {
        // Re-serialising exercises the writer against arbitrary parsed shapes, not just
        // the ones the encoder builds — and a value that parses but cannot be written is
        // itself a bug worth surfacing.
        let bytes = value.to_vec();

        // Re-encoding is idempotent: what the writer emits must parse, and writing that
        // again must produce identical bytes.
        //
        // Compared as *bytes*, not as values. `Value` derives `PartialEq`, so a payload
        // carrying a NaN float compares unequal to itself — which the fuzzer found within
        // three minutes, correctly reporting a round trip whose two sides print
        // identically. The bytes are the invariant that actually holds.
        match lcr_core::cbor::Value::from_slice(&bytes) {
            Ok(again) => assert_eq!(
                bytes,
                again.to_vec(),
                "re-encoding a parsed value was not idempotent"
            ),
            Err(e) => panic!("re-encoded cbor no longer parses: {e:?}"),
        }

        // Rendering to JSON is what a scanner does to show a payload, so it runs on
        // whatever the parser accepted.
        let _ = lcr_core::json::to_json(&value);
    }
});
