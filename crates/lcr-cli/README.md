# lcr-cli

The `lcr` command.

```bash
cargo install lcr-cli

lcr encode -d '{"name":"Fred"}' -o fred.png  # also .svg, or .gdf for a GDF document
lcr decode photo.jpg                         # prints {"name":"Fred"}
lcr decode --combine f1.png f2.png f3.png    # combine frames of one symbol
lcr capacity                                 # bytes per version and profile
lcr send report.pdf -o symbols/              # a file as a sequence of symbols
lcr receive symbols/*.png                    # any order; names what is missing
```

`lcr decode` reads PNG, JPEG, WebP, JPEG XL and AVIF, identified by content rather than file
name, in pure Rust. Formats it cannot read, such as HEIC, get a ready-made command to convert
them.

**Building on x86 or x86_64 needs [`nasm`](https://www.nasm.us/)** for the AVIF decoder's
assembly (`brew install nasm`, `apt install nasm`). Build without AVIF with
`cargo install lcr-cli --no-default-features --features png,jpeg,webp,jxl`. Requires Rust 1.98.

---

Part of [LCR Code](https://github.com/TheRincon/lcr-codes): a colour 2D barcode for offline data transfer — structured,
JSON-shaped payloads at higher density than a monochrome code, read from a screen or from
print with no network involved. See the [specification](https://github.com/TheRincon/lcr-codes/blob/main/docs/lcr-code-spec-v0.4.md)
and the [live demo](https://therincon.github.io/lcr-codes/).

Licensed under your choice of [MIT](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-MIT),
[Apache-2.0](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-APACHE) or the [Unlicense](https://github.com/TheRincon/lcr-codes/blob/main/UNLICENSE).
