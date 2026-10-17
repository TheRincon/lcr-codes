# LCR Code example symbols

Generated, not hand-made. Regenerate with:

```
just gallery
```

Each entry is a `.png` (12 device pixels per module, §13.1's preferred range), the same symbol as `.svg` and as a one-page [GDF](https://github.com/TheRincon/gdf) document (`.gdf`), and the source `.json`. Every one is decoded before it is written, so anything here reads back to the JSON beside it.

Payloads travel as CBOR, not JSON — §9.4's "author in JSON, transmit CBOR". The two size columns are that conversion. **Layout** is how a symbol is found: **Ring** is finder-less (D17), located and described entirely by its §6.4 ring; **Finders** is the classic layout.

**New to the format?** [`anatomy-c3.png`](anatomy-c3.png) boxes and explains every part of a C3 symbol: corners, sequence, version and format bits, references and payload. Regenerate it with `just anatomy`.

## Payloads

| Symbol | Profile | Layout | Version | Side | JSON | CBOR | What it is |
|---|---|---|---|---|---|---|---|
| [`contact`](contact.png) | C1 | Ring | v1 | 21×21 | 34 B | 26 B | The §6.1 worked example. |
| [`location-fix`](location-fix.png) | C1 | Ring | v1 | 21×21 | 52 B | 39 B | A position with accuracy and a timestamp. |
| [`allergen-profile`](allergen-profile.png) | C1 | Ring | v1 | 21×21 | 66 B | 53 B | Appendix A, written out as JSON rather than schema-coded — the size argument the schema registry (§11) exists to win back. |
| [`device-pairing`](device-pairing.png) | C1 | Ring | v1 | 21×21 | 67 B | 52 B | Wi-Fi handoff with no network in between, which is §1.1 goal 1 exactly. |
| [`membership`](membership.png) | C1 | Ring | v1 | 21×21 | 89 B | 61 B | Unsigned (§10.7): the holder asserts it, nobody attests it. |
| [`device-config`](device-config.png) | C1 | Ring | v3 | 29×29 | 144 B | 114 B | Nested objects and a float array — provisioning a device off a screen. |
| [`event-admission`](event-admission.png) | C1 | Ring | v2 | 25×25 | 150 B | 102 B | The shape that would carry a COSE signature (§10) in a deployment that needed one. |

## The same payload at every profile

`event-admission`, encoded four ways. This is §5's density trade made visible: the monochrome fallback needs a visibly bigger symbol for the same bytes — partly because it carries one bit per module where C1 carries two, and partly because M keeps its finders even with the ring. Its format cannot ride on hue, because it has none.

| Symbol | Profile | Layout | Version | Side | Modules | What it is |
|---|---|---|---|---|---|---|
| [`profile-m`](profile-m.png) | M | Finders | v6 | 41×41 | 1681 | Monochrome fallback (§4.6). Luma layer only — this is what a mono sensor or an unrecoverable chroma layer leaves you. |
| [`profile-c1`](profile-c1.png) | C1 | Ring | v2 | 25×25 | 625 | Screen default: 4 colours, co-sited chroma, 2.00 bits/module. |
| [`profile-c2`](profile-c2.png) | C2 | Ring | v4 | 33×33 | 1089 | Print default: 4 colours, 2×2 chroma cells, more robust and less dense. |
| [`profile-c3`](profile-c3.png) | C3 | Ring | v1 | 21×21 | 441 | 8 colours, 2×2 cells. Optional (§5) and wants a good capture. |

## With the ring, and without

A colour symbol that carries the §6.4 ring has **no finders, separators, palette anchors, alignment marker, timing or format region** (D17). The ring's four forced-dark corners give the homography; its luma position sequence names the version, the rotation and the mirror; and the format codeword rides on the colour choices of the ring positions between its fixed-colour references. Those structures were 204 modules at every version — 46% of a v1 — and all of them go back to payload.

Without the ring a symbol keeps the classic layout, and that is still the right choice wherever a capture might be greyscale: a finder-less symbol's format is carried in hue, so a mono sensor can locate one but cannot read it.

These are the same payloads as their ringed twins above.

| Symbol | Profile | Layout | Version | Side | Ringed twin | JSON | CBOR |
|---|---|---|---|---|---|---|---|
| [`contact-no-border`](contact-no-border.png) | C1 | Finders | v2 | 25×25 | [v1 21×21](contact.png) | 34 B | 26 B |
| [`event-admission-no-border`](event-admission-no-border.png) | C1 | Finders | v4 | 33×33 | [v2 25×25](event-admission.png) | 150 B | 102 B |

## Reading one back

```rust
let px = /* RGB8 bytes from a camera frame or a decoded PNG */;
let out = lcr_decode::Decoder::new().decode_rgb(&px, w, h, w * 3)?;
if let lcr_core::Packet::Unsigned { body, .. } = out.packet {
    let value = lcr_core::cbor::Value::from_slice(&body)?;
    println!("{}", lcr_core::json::to_json(&value));
}
```

All of these are **unsigned** (§10.7). Signing is opt-in and §1.2 expects unsigned structured data to be the common case; `lcr-verify` is not built yet in any event.
