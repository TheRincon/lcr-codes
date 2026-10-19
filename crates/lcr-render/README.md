# lcr-render

Draws an LCR Code symbol, quiet zone and reference ring included, as:

- an RGB raster or PNG, at an integer number of pixels per module;
- a compact SVG: one path per colour, about 8× smaller than one rectangle per run, and
  drawn exactly by every renderer;
- a one-page [GDF](https://github.com/TheRincon/gdf) document;
- ANSI colour text, for terminals and test output.

`CellGrid` exposes the symbol as a palette plus one index per cell, for writers of other
formats. The optional `qr` feature renders a QR code beside a symbol (§17.2).

---

Part of [LCR Code](https://github.com/TheRincon/lcr-codes): a colour 2D barcode for offline data transfer — structured,
JSON-shaped payloads at higher density than a monochrome code, read from a screen or from
print with no network involved. See the [specification](https://github.com/TheRincon/lcr-codes/blob/main/docs/lcr-code-spec-v0.4.md)
and the [live demo](https://therincon.github.io/lcr-codes/).

Licensed under your choice of [MIT](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-MIT),
[Apache-2.0](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-APACHE) or the [Unlicense](https://github.com/TheRincon/lcr-codes/blob/main/UNLICENSE).
