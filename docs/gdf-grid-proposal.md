# GDF proposal: the `grid` command and `palette` style

**Status:** proposal against `%GDF-0.3`. Written in the LCR Code repository, where the need
arose, for transfer to the GDF specification. Section references (§n) are to
`gdf-spec_v0.3.md`.

**Summary:** add one artifact command, `grid`, which paints a rectangle divided into
equal cells, each one colour from a named `palette` style. Cells are point-sampled, never
filtered, so a grid renders exactly at every size. It is the missing primitive for 2D
symbologies (LCR Code, QR, Data Matrix), pixel art, heat maps and similar figures.

---

## 1. Motivation

GDF 0.3 has two ways to draw a grid of coloured cells, and neither fits.

- **Paths** (§10.3) are exact but describe *shapes*. A colour symbol is close to noise —
  in a typical LCR Code three runs in four are one cell long, and no run spans two rows —
  so geometry costs bytes per cell however it is merged, plus a style per colour.
- **Images** (§8.1) are compact, but §13 leaves filtering unspecified. A small bitmap
  stretched to its box is smoothed by some renderers, which blurs every cell edge. For a
  machine-read symbol that costs decode margin; for pixel art it is simply wrong.

Measured on one LCR Code (`profile-c1`: 35 × 35 cells, 6 colours), every document drawing
the identical page:

| Encoding | Text | zstd -19 | Exact? |
|---|---:|---:|---|
| `path` per colour, stroked runs (best GDF 0.3 vector) | 5,322 B | 1,258 B | yes |
| `[image]` PNG, one sample per cell, `fit:fill` | 809 B | 767 B | **no** — renderer-dependent smoothing |
| `grid` (this proposal) | 1,364 B | 513 B | yes |

The grid is the smallest exact form once packed, and the only one that is both exact and
readable.

## 2. `palette` styles (addition to §6)

A fourth style kind, beside `text`, `paint` and `table`:

```
  palette <name> colors:<list>
```

| Attribute | Meaning |
|---|---|
| `colors` | A list (§3.3) of 1 to 36 colours (§3.3), in index order: the first is index `0`. |

- `colors` is required.
- Entries **MUST** be distinct.
- Colours may carry alpha (`#rrggbbaa`). Painting composites as §13 specifies; a fully
  transparent entry lets the page show through.
- Names share the single style namespace (§6.1).
- In canonical form:
  - palettes follow the `table` styles;
  - palettes are ordered by first reference, like every other kind;
  - every entry is used by at least one grid that references the palette.

## 3. The `grid` command (addition to §10.3)

| Command | Syntax |
|---|---|
| Grid | `grid s:<palette> box:<x>,<rel>,<w>,<h> cells:<cols>x<rows> <string>` |

- `grid` is an **artifact**: it is positioned relative to the flow cursor like `rect`, does
  not move the cursor, and is skipped by text extraction (§14).
- `cells` gives the number of columns and rows. Both are positive integers.
- The string holds **one character per cell**, row-major, left to right and top to bottom.
  - Each character is a base-36 digit, `0`–`9` then `a`–`z`, lower case only. It is an
    index into the palette.
  - The string's length **MUST** be exactly `cols × rows`.
  - Every digit **MUST** be less than the palette's length.
  - No other characters are allowed: no separators, no whitespace, no escapes.

Example: a 3 × 2 grid in two colours, 30 × 20 units:

```
[styles]
  palette ink colors:#ffffff,#1a1a2e

[page 1 size:100x100]
  grid s:ink box:10,+10,30,20 cells:3x2 "010101"
```

## 4. Painting (addition to §13)

Let the box occupy `[X0, X1) × [Y0, Y1)` in grid units (§2.3), with `W = X1 − X0` and
`H = Y1 − Y0`.

- **Cell edges are integers.** Column `i` spans `[X0 + floor(i·W / cols), X0 + floor((i+1)·W / cols))`, and row `j` likewise with `H` and `rows`. Adjacent cells therefore share an
  edge exactly, with no gap and no overlap.
- **Cells are point-sampled.** A device pixel whose centre lies inside the box takes the
  colour of the cell that contains that centre, composited over what is beneath it (§13).
  No filtering, smoothing or anti-aliasing is applied between cells.
- Pixels straddling the box's **outer** edge may be anti-aliased as a `rect` would be.
- A renderer **MUST NOT** draw seams between cells. Point sampling guarantees this; a
  renderer that tessellates cells into rectangles must achieve the same result.
- Grids are clipped by the page like every other command (§13).

*(Informative)* Point sampling is what makes the result renderer-independent. Two
conforming renderers at the same device resolution produce identical pixels inside the
box. That is the property a printed or displayed barcode needs, and the one §8.1 images
cannot promise.

## 5. Validation (additions to §19.3)

A validator reports:

- an unknown palette, or a style of another kind named by `s:`;
- a string whose length is not `cols × rows`;
- a character that is not a lower-case base-36 digit;
- a digit not less than the palette's length;
- a non-positive `w`, `h`, `cols` or `rows`;
- cells too small to render: fewer than one device pixel per cell at the document's
  intended resolution. This is a warning, not an error.

## 6. Limits (additions to §18.6)

| Item | Limit |
|---|---|
| Palette entries | 36 |
| Cells per grid | 262,144, which §18.6's string limit already implies |

A 512 × 512 grid fits. Larger grids belong in an `[image]` block, where filtering is
acceptable.

## 7. Versioning

`grid` and `palette` change core rendering, so they cannot be an `x-` extension (§19.4).
They require a new draft version on the magic line. A writer that must target `%GDF-0.3`
uses stroked `path` runs, as LCR Code's `render_gdf` does today, and switches to `grid`
when the version is available.

## 8. Open questions

1. **Accessibility.** As an artifact, a grid carries no text. A code on a ticket has a
   meaning a screen reader should announce. One option is a content variant with
   `alt:<string>` that advances the cursor like `img` (§10.1). Another is to keep `grid`
   an artifact and let authors place a caption.
2. **Denser encodings.** One character per cell is readable and packs to near entropy
   under zstd (§16). A base64 packed form (`bits:2` for 4-colour palettes) would cut
   unpacked size by 4× at the cost of readability. Recommended only if unpacked size
   matters in practice.
3. **Non-square cells.** These fall out of `W/cols ≠ H/rows` already. Nothing more is
   needed unless a use appears.

## Appendix: the LCR Code example

`examples/profile-c1` from this repository, at 12 GDF px per module (3 mm), as the gallery
renders it:

```
[styles]
  palette lcr colors:#ffffff,#8bd377,#faa2fb,#025901,#78297c,#000000

[page 1 size:420x420]
  grid s:lcr box:0,+0,420,420 cells:35x35 "00000000000000000000000000000000000…"
```

The full string is 1,225 digits. The same symbol as a valid `%GDF-0.3` vector is
[`examples/profile-c1.gdf`](../examples/profile-c1.gdf).
