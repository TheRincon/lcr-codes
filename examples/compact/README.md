# Compact encodings — a comparison

One symbol, `profile-c1` (35 × 35 modules with quiet zone and ring, 6 colours), written six
ways. Every file here was rendered — the SVGs by headless Chrome, the GDFs by GDF's own
renderer — and the pixels decoded back to the payload with `lcr decode`.

`paths.svg` and `profile-c1.gdf` are now what `lcr_render::render_svg` and `render_gdf`
write, and every symbol in [`examples/`](..) ships as both. The others are kept for the
comparison.

| File | Form | Bytes | zstd -19 |
|---|---|---:|---:|
| `profile-c1.rects.svg` | today's `render_svg`: one `<rect>` per run | 38,168 | 2,011 |
| `profile-c1.paths.svg` | one stroked `<path>` per colour, relative moves | 4,558 | 1,049 |
| `profile-c1.png-in-svg.svg` | one `<image>`: indexed PNG, 1 px per module | 753 | 624 |
| `profile-c1.gdf` | GDF 0.3, one stroked `path` per colour | 5,322 | 1,258 (`.gdfz`) |
| `profile-c1.raster.gdf` | GDF 0.3, `[image]` PNG stretched to the page | 809 | 767 (`.gdfz`) |

## Why geometry is the wrong tool

A colour symbol is close to noise: of `profile-c1`'s 661 runs, 517 are one module long and
none is taller than one row. Anything that describes *shapes* pays per module — about 58
bytes in `rects.svg`, 7 in `paths.svg` — and repeats a colour on every one. The information
is only ~2.6 bits per module (6 colours), which is why a palette plus one index per module
is 6–50× smaller than any vector form.

## What each form is good for

- **`paths.svg`** is the best *portable vector* SVG. Every renderer draws it exactly, and
  it is 8× smaller than today's output with no change in appearance. The tricks:
  - each colour is stated once, on its path;
  - runs are horizontal strokes 1 module wide, so a run costs `m4 0h2`, not a rectangle;
  - a `translate(0 .5)` puts every stroke on its row's centre line;
  - the commonest colour is the background, so its runs are never drawn.
- **`png-in-svg.svg`** is the smallest SVG, but its crispness depends on the renderer
  honouring `image-rendering: pixelated`. Browsers do; not every SVG tool does, and a
  smoothed symbol loses decode margin.
- **`profile-c1.gdf`** is the same stroked-path idea in GDF 0.3, and is valid today.
- **`profile-c1.raster.gdf`** is 6× smaller, but GDF leaves image filtering unspecified,
  and GDF's renderer smooths the stretched bitmap. It still decodes, with less margin.

## A proposal for GDF: a `grid` command

Neither format has a primitive for "a grid of coloured cells, drawn exactly". A small one
would serve LCR Codes, QR codes, pixel art and heat maps alike:

```
[styles]
  palette lcr #ffffff,#8bd377,#faa2fb,#025901,#78297c,#000000

[page 1 size:280x280]
  grid s:lcr box:0,+0,280,280 cells:35x35 "0000000…"
```

- One base-36 digit per cell, row-major, so the string is `cols × rows` characters long and
  still readable.
- Cells are drawn as exact, seamless axis-aligned squares. No filtering exists to vary.
- About 1.4 KB as text for this symbol, and about 0.5 KB with zstd.
- It is an artifact, like `rect`.

This is not valid GDF 0.3. It would need a new draft version, gated by the magic line
(§19.4).
