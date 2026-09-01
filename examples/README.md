# LC Code example symbols

Generated, not hand-made. Regenerate with:

```
cargo run -p lc-render --release --example gallery -- examples
```

Each entry is a `.png` (12 device pixels per module, §13.1's preferred range), the same symbol as `.svg`, and the source `.json`. Every one is decoded before it is written, so anything here reads back to the JSON beside it.

Payloads travel as CBOR, not JSON — §9.4's "author in JSON, transmit CBOR". The two size columns are that conversion.

## Payloads

| Symbol | Profile | Version | Side | JSON | CBOR | What it is |
|---|---|---|---|---|---|---|
| [`contact`](contact.png) | C1 | v2 | 25×25 | 34 B | 26 B | The §6.1 worked example. |
| [`location-fix`](location-fix.png) | C1 | v2 | 25×25 | 54 B | 39 B | A position with accuracy and a timestamp. |
| [`allergen-profile`](allergen-profile.png) | C1 | v3 | 29×29 | 66 B | 53 B | Appendix A, written out as JSON rather than schema-coded — the size argument the schema registry (§11) exists to win back. |
| [`device-pairing`](device-pairing.png) | C1 | v3 | 29×29 | 67 B | 52 B | Wi-Fi handoff with no network in between, which is §1.1 goal 1 exactly. |
| [`membership`](membership.png) | C1 | v3 | 29×29 | 96 B | 68 B | Unsigned (§10.7): the holder asserts it, nobody attests it. |
| [`device-config`](device-config.png) | C1 | v4 | 33×33 | 144 B | 114 B | Nested objects and a float array — provisioning a device off a screen. |
| [`event-admission`](event-admission.png) | C1 | v4 | 33×33 | 144 B | 96 B | The shape that would carry a COSE signature (§10) in a deployment that needed one. |

## The same payload at every profile

`event-admission`, encoded four ways. This is §5's density trade made visible: the monochrome fallback needs a visibly bigger symbol for the same bytes.

| Symbol | Profile | Version | Side | Modules | What it is |
|---|---|---|---|---|---|
| [`profile-m`](profile-m.png) | M | v6 | 41×41 | 1681 | Monochrome fallback (§4.6). Luma layer only — this is what a mono sensor or an unrecoverable chroma layer leaves you. |
| [`profile-c1`](profile-c1.png) | C1 | v4 | 33×33 | 1089 | Screen default: 4 colours, co-sited chroma, 2.00 bits/module. |
| [`profile-c2`](profile-c2.png) | C2 | v5 | 37×37 | 1369 | Print default: 4 colours, 2×2 chroma cells, more robust and less dense. |
| [`profile-c3`](profile-c3.png) | C3 | v5 | 37×37 | 1369 | 8 colours, 2×2 cells. Optional (§5) and wants a good capture. |

## Without the reference border

The §6.4 ring is **optional** — signalled by format bit 14, and these two decode without it. It lives outside the symbol grid, so removing it changes nothing about what is encoded: same payload, same version, same module pattern. Compare each against its bordered twin above.

It is not free to drop, though. Measured over a 120-case tilt × glare grid, decoding succeeds in 65 cases with the border and 53 without — the ring gives the §12.3 illumination fit known-colour samples at the geometric outside of the symbol, which is where a global fit is weakest and where glare gradients are strongest. §6.4's own note is that on screen it costs pixels nobody was using; in print the area would not be worth it.

| Symbol | Profile | Version | Side | JSON | CBOR |
|---|---|---|---|---|---|
| [`contact-no-border`](contact-no-border.png) | C1 | v2 | 25×25 | 34 B | 26 B |
| [`event-admission-no-border`](event-admission-no-border.png) | C1 | v4 | 33×33 | 144 B | 96 B |

## Reading one back

```rust
let px = /* RGB8 bytes from a camera frame or a decoded PNG */;
let out = lc_decode::Decoder::new().decode_rgb(&px, w, h, w * 3)?;
if let lc_core::Packet::Unsigned { body, .. } = out.packet {
    let value = lc_core::cbor::Value::from_slice(&body)?;
    println!("{}", lc_core::json::to_json(&value));
}
```

All of these are **unsigned** (§10.7). Signing is opt-in and §1.2 expects unsigned structured data to be the common case; `lc-verify` is not built yet in any event.
