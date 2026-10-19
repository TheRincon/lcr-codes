# lcr-core

The shared foundation of LCR Code: symbol geometry, the colour palettes and colour
science, LDPC error correction, framing, masking and modulation. Everything the encoder and
decoder must agree on lives here, once.

Most users want [`lcr-encode`](https://crates.io/crates/lcr-encode) and
[`lcr-decode`](https://crates.io/crates/lcr-decode) rather than this crate directly.

---

Part of [LCR Code](https://github.com/TheRincon/lcr-codes): a colour 2D barcode for offline data transfer — structured,
JSON-shaped payloads at higher density than a monochrome code, read from a screen or from
print with no network involved. See the [specification](https://github.com/TheRincon/lcr-codes/blob/main/docs/lcr-code-spec-v0.4.md)
and the [live demo](https://therincon.github.io/lcr-codes/).

Licensed under your choice of [MIT](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-MIT),
[Apache-2.0](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-APACHE) or the [Unlicense](https://github.com/TheRincon/lcr-codes/blob/main/UNLICENSE).
