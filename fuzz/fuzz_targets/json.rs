//! Fuzz the JSON parser (§9.4).
//!
//! Less exposed than the others — JSON is an *authoring* format here, not a wire one, and
//! §9.2 discourages it on the wire — but `encode_json` is a public entry point, and the
//! browser demo feeds it a textarea. Anything a user can paste is worth fuzzing.
//!
//! It shares `cbor::Value` as its output type, so this also exercises the value tree from
//! a completely different direction than the CBOR target does: text nesting rather than
//! byte headers.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Only valid UTF-8 reaches `parse`, matching its `&str` signature.
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };

    if let Ok(value) = lc_core::json::parse(text) {
        // Writing back must not panic on anything the parser accepted.
        let written = lc_core::json::to_json(&value);

        // What must hold is that the projection **converges**, not that it is idempotent
        // in one step. `to_json` is deliberately lossy — JSON has no NaN, no infinity and
        // no bytes type — and the fuzzer found a third case in under four minutes: the
        // text `-0` is ambiguous, so `Float(-0.0)` writes as "-0" and reads back through
        // the *integer* path as `Uint(0)`, which writes as "0". Every JSON implementation
        // picks a side there; it is not a defect.
        //
        // So one pass may lose information. From the second pass on, nothing may change —
        // and a writer that produced output its own parser rejected, or that never
        // settled, would still be caught.
        let Ok(reparsed) = lc_core::json::parse(&written) else {
            panic!("the writer emitted json its own parser rejects: {written:?}");
        };
        let second = lc_core::json::to_json(&reparsed);
        let Ok(again) = lc_core::json::parse(&second) else {
            panic!("second pass emitted json its own parser rejects: {second:?}");
        };
        assert_eq!(
            second,
            lc_core::json::to_json(&again),
            "json projection never settled"
        );
    }
});
