# lcr-decode

Reads LCR Code symbols from camera frames: detection, perspective correction, a fitted
illumination model, soft demodulation and LDPC decoding. Accepts RGB, RGBA or greyscale
buffers with any stride, reports diagnostics (SNR, capture geometry, whether colour was
recovered) for user guidance, and combines several frames of the same symbol (§12.4) when
one capture is not enough.

```rust
use lcr_encode::EncodeOptions;
use lcr_render::{RenderOptions, render_rgb};
use lcr_decode::Decoder;

// Author in JSON, transmit CBOR (§9.4).
let sym = lcr_encode::encode_json(r#"{"name":"Fred"}"#, &EncodeOptions::screen())?;
let img = render_rgb(&sym, &RenderOptions::screen());

let out = Decoder::new().decode_rgb(&img.data, img.width, img.height, img.width * 3)?;
```

---

Part of [LCR Code](https://github.com/TheRincon/lcr-codes): a colour 2D barcode for offline data transfer — structured,
JSON-shaped payloads at higher density than a monochrome code, read from a screen or from
print with no network involved. See the [specification](https://github.com/TheRincon/lcr-codes/blob/main/docs/lcr-code-spec-v0.4.md)
and the [live demo](https://therincon.github.io/lcr-codes/).

Licensed under your choice of [MIT](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-MIT),
[Apache-2.0](https://github.com/TheRincon/lcr-codes/blob/main/LICENSE-APACHE) or the [Unlicense](https://github.com/TheRincon/lcr-codes/blob/main/UNLICENSE).
