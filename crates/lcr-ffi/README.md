# lcr-ffi

A C API for LCR Code, built as a static or shared library (`liblcr_ffi`) with a generated
header, `include/lcr.h`. It covers decoding, probing, multi-frame combining, encoding to RGB
or SVG, capacity lookup, CBOR/JSON conversion, and CBOR File Transfer.

Every call returns a status code, `lcr_last_error()` explains a failure, and everything the
library returns has a matching `_free` function. The shortest complete example is
[`tests/smoke.c`](https://github.com/TheRincon/lcr-codes/blob/main/crates/lcr-ffi/tests/smoke.c).

---

Part of [LCR Code](https://github.com/TheRincon/lcr-codes): a colour 2D barcode for offline data transfer — structured,
JSON-shaped payloads at higher density than a monochrome code, read from a screen or from
print with no network involved. See the [specification](https://github.com/TheRincon/lcr-codes/blob/main/docs/lcr-code-spec-v0.4.md)
and the [live demo](https://therincon.github.io/lcr-codes/).

Licensed under your choice of [MIT](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-MIT),
[Apache-2.0](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-APACHE) or the [Unlicense](https://github.com/TheRincon/lcr-codes/blob/main/UNLICENSE).
