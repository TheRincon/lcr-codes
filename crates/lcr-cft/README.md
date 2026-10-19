# lcr-cft

CBOR File Transfer: send a file through any carrier that moves small messages, such as a
sequence of LCR Code symbols. The file is compressed once, split into chunks sized to the
carrier, and described by a manifest with its hash. A receiver accepts messages in any
order, reports which chunks are missing, and returns the file only when its hash matches.
Size limits are checked before anything is allocated, so a hostile manifest cannot exhaust
memory.

Deflate is pure Rust and always available. The default `zstd` feature adds Zstandard, which
compiles C.

---

Part of [LCR Code](https://github.com/TheRincon/lcr-codes): a colour 2D barcode for offline data transfer — structured,
JSON-shaped payloads at higher density than a monochrome code, read from a screen or from
print with no network involved. See the [specification](https://github.com/TheRincon/lcr-codes/blob/main/docs/lcr-code-spec-v0.4.md)
and the [live demo](https://therincon.github.io/lcr-codes/).

Licensed under your choice of [MIT](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-MIT),
[Apache-2.0](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-APACHE) or the [Unlicense](https://github.com/TheRincon/lcr-codes/blob/main/UNLICENSE).
