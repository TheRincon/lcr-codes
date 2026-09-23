//! Generate `examples/anatomy-c3.png`: a C3 finder-less symbol with every structural part
//! boxed in red and explained in a numbered legend.
//!
//! `just anatomy`
//!
//! This writes an HTML page and the symbol PNG into a build directory, and the recipe turns
//! the page into a PNG with headless Chrome — nothing in the workspace renders text.
//!
//! Every box is computed from lc-core's own geometry — `border::position`,
//! `border::format_slot`, the sequence window, the layout's module roles — so the figure
//! cannot drift from the code. The symbol is decoded before anything is written.

use lc_core::border::{self, GAP, WIDTH};
use lc_core::layout::Layout;
use lc_core::{ModuleRole, Packet, Profile};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{Canvas, RenderOptions, render_png, render_rgb};
use std::fmt::Write as _;

/// The gallery's `event-admission` payload, so the figure matches `profile-c3.png`.
const JSON: &str = r#"{"event":"EV-2026-0417","seat":{"sec":"104","row":"F","num":12},"holder":"A. N. Other","tier":"ga","gates":[2,3],"issued":1756512000,"exp":1776988800}"#;

/// Device pixels per module in the figure.
const PX: f32 = 24.0;
/// White space around the rendered canvas, for badges and the direction arrow.
const PAD: f32 = 64.0;
/// Page size. `just anatomy` passes the same numbers to Chrome's `--window-size`.
const PAGE: (u32, u32) = (1900, 1170);
const RED: &str = "#D0021B";

/// A rectangle in module coordinates: the symbol grid spans `0..side` on both axes.
#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn cell(x: i32, y: i32) -> Rect {
        Rect {
            x: x as f32,
            y: y as f32,
            w: 1.0,
            h: 1.0,
        }
    }

    fn union(a: Rect, b: Rect) -> Rect {
        let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
        let (x1, y1) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    fn inflate(self, d: f32) -> Rect {
        Rect {
            x: self.x - d,
            y: self.y - d,
            w: self.w + 2.0 * d,
            h: self.h + 2.0 * d,
        }
    }

    fn centre(self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// The point of this rectangle closest to `p`.
    fn nearest(self, p: (f32, f32)) -> (f32, f32) {
        (
            p.0.clamp(self.x, self.x + self.w),
            p.1.clamp(self.y, self.y + self.h),
        )
    }
}

/// Boxes that share a badge, or carry none when another group of the same note has it.
struct Group {
    badge: Option<(f32, f32)>,
    boxes: Vec<Rect>,
}

struct Note {
    title: String,
    body: String,
    groups: Vec<Group>,
    dashed: bool,
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/anatomy".into());
    std::fs::create_dir_all(&dir).expect("create output directory");

    let opts = EncodeOptions {
        profile: Profile::C3,
        reference_border: true,
        ..Default::default()
    };
    let sym = lc_encode::encode_json(JSON, &opts).expect("encode");
    let cbor = lc_core::json::parse(JSON).unwrap().to_vec();
    let render = RenderOptions {
        module_px: PX as u32,
        ..RenderOptions::screen()
    };

    // Nothing gets annotated that does not decode.
    let raster = render_rgb(&sym, &render);
    let out = Decoder::new()
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .unwrap_or_else(|e| panic!("anatomy symbol does not decode: {e}"));
    match &out.packet {
        Packet::Unsigned { body, .. } => {
            let back = lc_core::cbor::Value::from_slice(body).expect("valid CBOR");
            assert_eq!(lc_core::json::to_json(&back), JSON, "round trip");
        }
        other => panic!("{other:?}"),
    }

    let f = sym.format;
    assert!(
        Layout::is_finderless(f.profile, f.reference_border),
        "the figure explains the finder-less layout"
    );
    let (version, side) = (f.version.get(), f.version.side());
    let layout = Layout::for_symbol(f.version, f.profile, f.reference_border);
    let palette = sym.palette();
    let hues = palette.hues;
    let bpp = border::bits_per_position(hues);
    let margin = Canvas::of(&sym).margin as f32;
    let window = lc_core::border_seq::WINDOW[version as usize - 1] as usize;

    // Ring geometry: the outer square runs `lo..=hi` on both axes, `s` cells a side, and
    // position `p` sits on edge `p / (s - 1)` (0 top, 1 right, 2 bottom, 3 left).
    let s = side as usize + 2 * (GAP + WIDTH) as usize;
    let (lo, hi) = (-(GAP + WIDTH), side as i32 + GAP + WIDTH - 1);
    let cells_of = |p: usize| -> Rect {
        border::cells(side)
            .filter(|&(x, y)| border::position(x, y, side) == Some(p))
            .map(|(x, y)| Rect::cell(x, y))
            .reduce(Rect::union)
            .expect("every ring position has a cell")
    };
    // Badges sit in the white margin just outside the ring.
    let (l, h) = (lo as f32 - 0.95, hi as f32 + 1.95);
    let by_edge = |ps: &[usize]| -> Vec<Group> {
        (0..4)
            .filter_map(|e| {
                let boxes: Vec<Rect> = ps
                    .iter()
                    .filter(|&&p| p / (s - 1) == e)
                    .map(|&p| cells_of(p))
                    .collect();
                if boxes.is_empty() {
                    return None;
                }
                let n = boxes.len() as f32;
                let (cx, cy) = boxes
                    .iter()
                    .map(|b| b.centre())
                    .fold((0.0, 0.0), |a, c| (a.0 + c.0 / n, a.1 + c.1 / n));
                let badge = match e {
                    0 => (cx, l),
                    1 => (h, cy),
                    2 => (cx, h),
                    _ => (l, cy),
                };
                Some(Group {
                    badge: Some(badge),
                    boxes,
                })
            })
            .collect()
    };

    let corners = [0, s - 1, 2 * (s - 1), 3 * (s - 1)];
    for &c in &corners {
        assert!(!border::sequence_light(side, c), "corner {c} must be dark");
    }

    // Format copy A: the version is the top five of its fifteen data bits.
    let slots = |bits: std::ops::Range<usize>| {
        let mut v: Vec<usize> = bits
            .map(|b| border::format_slot(0, b, side, hues).0)
            .collect();
        v.dedup();
        v
    };
    // Bits 0–3 of a ringed symbol's word are the profile (D21); the version is not in
    // the word at all, because the ring's sequence already gave it.
    let profile_pos = slots(0..4);
    let mut rest_pos = slots(4..15);
    rest_pos.retain(|p| !profile_pos.contains(p));

    let arc_start = 3 * (s - 1) + (s - 1) / 2 - window / 2;
    let arc = (arc_start..arc_start + window)
        .map(cells_of)
        .reduce(Rect::union)
        .unwrap();

    let refs: Vec<usize> = (2 * (s - 1) + (s - 1) / 2 - 4..)
        .filter(|&p| border::is_reference(p))
        .take(4)
        .collect();

    let interior: Vec<(u16, u16)> = (0..side)
        .flat_map(|y| (0..side).map(move |x| (x, y)))
        .filter(|&(x, y)| matches!(layout.roles.at(x, y), ModuleRole::Reference))
        .collect();

    let c = side as i32 / 2;
    let (mx, my) = (0..side as i32)
        .flat_map(|r| (-r..=r).flat_map(move |dy| (-r..=r).map(move |dx| (c + dx, c + dy))))
        .find(|&(x, y)| {
            (0..side as i32).contains(&x)
                && (0..side as i32).contains(&y)
                && matches!(layout.roles.at(x as u16, y as u16), ModuleRole::Payload)
                // Far enough from every interior reference that its box cannot be
                // mistaken for one of theirs.
                && interior
                    .iter()
                    .all(|&(rx, ry)| (rx as i32 - x).abs() > 2 || (ry as i32 - y).abs() > 2)
        })
        .expect("a payload module");

    let cap = layout.capacity();
    let sf = side as f32;
    let notes = vec![
        Note {
            title: "Ring corners (all four)".into(),
            body: "A dark hue or black in each corner. They fix the perspective transform, WITHOUT finder patterns \
                   needed. The top-left one is also part of note 2."
                .into(),
            groups: corners
                .iter()
                .zip([None, Some((h, l)), Some((h, h)), Some((l, h))])
                .map(|(&p, badge)| Group {
                    badge,
                    boxes: vec![cells_of(p)],
                })
                .collect(),
            dashed: false,
        },
        Note {
            title: "Top-left: the sequence starts here".into(),
            body: "Position 0 of the ring's light/dark sequence, which runs clockwise from \
                   here. The decoder tries every corner as the start, in both directions. The \
                   one that matches tells it which way is up, and whether the image is \
                   mirrored."
                .into(),
            groups: vec![Group {
                badge: Some((l, l)),
                boxes: vec![cells_of(0).inflate(0.22)],
            }],
            dashed: false,
        },
        Note {
            title: format!("Any {window} in a row are unique"),
            body: format!(
                "Every run of {window} consecutive light/dark positions occurs exactly once \
                 around the ring, so a clear stretch of ring knows where it sits even when the \
                 rest is hidden. Only the v{version} sequence lines up with this ring, which is \
                 the first reading of the version."
            ),
            groups: vec![Group {
                badge: Some((l, arc.centre().1)),
                boxes: vec![arc],
            }],
            dashed: false,
        },
        Note {
            title: format!("Profile bits: {:?}", f.profile),
            body: format!(
                "Four bits of the format codeword name the profile — room for 16, of which 4 \
                 are defined. Each non-reference ring position carries {bpp} format bits in \
                 its choice among {hues} hues, so these {} positions hold them (copy A). \
                 BCH(30,15) lets each copy survive 3 wrong bits. The version is *not* in the \
                 word: the sequence above already gave it, which is what freed these bits.",
                profile_pos.len()
            ),
            groups: by_edge(&profile_pos),
            dashed: false,
        },
        Note {
            title: "The rest of the format".into(),
            body: "The two ECC levels, the mask, and four reserved bits that must read zero \
                   — copy A. The other 15 bits of each copy are BCH parity. Copy B sits half a \
                   step along, so glare over one arc chips a few bits from both copies instead \
                   of erasing one."
                .into(),
            groups: by_edge(&rest_pos),
            dashed: false,
        },
        Note {
            title: "Reference cells".into(),
            body: format!(
                "Every {}rd ring position has a fixed colour — one of the {} palette colours, \
                 black or white — that doesn't depend on the data. The decoder fits \
                 illumination from these before it can read a single hue.",
                border::REF_STRIDE,
                palette.len()
            ),
            groups: by_edge(&refs),
            dashed: false,
        },
        Note {
            title: "Quiet zone".into(),
            body: format!(
                "{GAP} white module between the ring and the grid, and {} more outside the \
                 ring. It keeps the ring's outline separate from the data and from anything \
                 else in the scene.",
                margin as i32 - GAP - WIDTH
            ),
            groups: vec![Group {
                badge: Some((sf + GAP as f32 / 2.0, sf * 0.62)),
                boxes: vec![Rect {
                    x: sf,
                    y: 0.0,
                    w: GAP as f32,
                    h: sf,
                }],
            }],
            dashed: true,
        },
        Note {
            title: format!("Payload grid: {side} × {side}"),
            body: format!(
                "No finders, separators, alignment marker, timing pattern or format region. \
                 {} of the {} modules carry payload; the rest are interior references.",
                layout.payload.len(),
                side as usize * side as usize
            ),
            groups: vec![Group {
                badge: Some((-1.0, sf + 1.0)),
                boxes: vec![Rect {
                    x: 0.0,
                    y: 0.0,
                    w: sf,
                    h: sf,
                }],
            }],
            dashed: true,
        },
        Note {
            title: format!("One module = {} bits", 1 + f.profile.chroma_bits_per_cell()),
            body: format!(
                "1 luma bit (dark or light) plus {} chroma bits (which of {hues} hues): one of \
                 {:?}'s {} colours. Each layer has its own LDPC error correction, interleaved \
                 across the grid. This symbol holds {} luma + {} chroma bits before ECC.",
                f.profile.chroma_bits_per_cell(),
                f.profile,
                palette.len(),
                cap.luma_bits,
                cap.chroma_bits
            ),
            groups: vec![Group {
                badge: Some((mx as f32 + 2.4, my as f32 - 1.4)),
                boxes: vec![Rect::cell(mx, my).inflate(0.12)],
            }],
            dashed: false,
        },
        Note {
            title: "Ring payload cells".into(),
            body: format!(
                "A data position's inner cell is an ordinary payload module (D19): one luma \
                 bit plus {} chroma bits, carried in the tail of the same codewords the grid \
                 uses. {} of them here. Reference positions and the four corners keep both \
                 radial layers, which is what detection and the illumination fit are built on.",
                if f.profile.chroma_cell() == 1 {
                    f.profile.chroma_bits_per_cell()
                } else {
                    0
                },
                layout.ring_cells
            ),
            groups: {
                let boxes: Vec<Rect> = border::payload_cells(side)
                    .filter(|&(x, y)| x == lo + 1 && (sf as i32 - 7..sf as i32).contains(&y))
                    .map(|(x, y)| Rect::cell(x, y))
                    .collect();
                let cy = boxes.iter().map(|b| b.centre().1).sum::<f32>() / boxes.len() as f32;
                vec![Group {
                    badge: Some((l, cy)),
                    boxes,
                }]
            },
            dashed: false,
        },
        Note {
            title: "Interior references".into(),
            body: format!(
                "{} modules of known colour scattered through the grid so the glare can be corrected.",
                interior.len()
            ),
            groups: interior
                .iter()
                .enumerate()
                .map(|(i, &(x, y))| Group {
                    badge: (i == 0).then_some((x as f32 - 1.3, y as f32 - 1.3)),
                    boxes: vec![Rect::cell(x as i32, y as i32).inflate(0.08)],
                })
                .collect(),
            dashed: false,
        },
    ];

    // ------------------------------------------------------------ overlay
    let canvas = (sf + 2.0 * margin) * PX;
    let fig = canvas + 2.0 * PAD;
    let at = |v: f32| PAD + (v + margin) * PX;
    let red = RED;

    let mut svg = String::new();
    write!(
        svg,
        r#"<svg width="{fig}" height="{fig}" viewBox="0 0 {fig} {fig}" xmlns="http://www.w3.org/2000/svg"><defs><marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L10,5 L0,10 z" fill="{red}"/></marker></defs>"#
    )
    .unwrap();

    for note in &notes {
        let dash = if note.dashed {
            r#" stroke-dasharray="12 7""#
        } else {
            ""
        };
        for b in note.groups.iter().flat_map(|g| &g.boxes) {
            let b = b.inflate(0.05);
            write!(
                svg,
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="none" stroke="{red}" stroke-width="3"{dash}/>"#,
                at(b.x),
                at(b.y),
                b.w * PX,
                b.h * PX
            )
            .unwrap();
        }
    }
    for g in notes.iter().flat_map(|n| &n.groups) {
        let Some(badge) = g.badge else { continue };
        for b in &g.boxes {
            let n = b.inflate(0.05).nearest(badge);
            if (n.0 - badge.0).hypot(n.1 - badge.1) > 0.7 {
                write!(
                    svg,
                    r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{red}" stroke-width="2"/>"#,
                    at(badge.0),
                    at(badge.1),
                    at(n.0),
                    at(n.1)
                )
                .unwrap();
            }
        }
    }

    // The direction the sequence runs, above the top-left of the ring.
    let (ay, ax0, ax1) = (at(-margin) - 22.0, at(lo as f32), at(lo as f32 + 10.0));
    write!(
        svg,
        r#"<line x1="{ax0:.1}" y1="{ay:.1}" x2="{ax1:.1}" y2="{ay:.1}" stroke="{red}" stroke-width="3" marker-end="url(#arrow)"/><text x="{:.1}" y="{:.1}" fill="{red}" font-family="-apple-system,Helvetica Neue,Helvetica,Arial,sans-serif" font-size="15" font-weight="600">sequence runs clockwise</text>"#,
        ax1 + 14.0,
        ay + 5.0
    )
    .unwrap();

    for (i, note) in notes.iter().enumerate() {
        for badge in note.groups.iter().filter_map(|g| g.badge) {
            write!(
                svg,
                r#"<circle cx="{:.1}" cy="{:.1}" r="15" fill="{red}" stroke="white" stroke-width="2.5"/><text x="{:.1}" y="{:.1}" text-anchor="middle" dominant-baseline="central" fill="white" font-family="-apple-system,Helvetica Neue,Helvetica,Arial,sans-serif" font-size="16" font-weight="700">{}</text>"#,
                at(badge.0),
                at(badge.1),
                at(badge.0),
                at(badge.1) + 0.5,
                i + 1
            )
            .unwrap();
        }
    }
    svg.push_str("</svg>");

    // ------------------------------------------------------------ page
    let mut legend = String::new();
    for (i, n) in notes.iter().enumerate() {
        write!(
            legend,
            r#"<li><span class="badge">{}</span><div><h3>{}</h3><p>{}</p></div></li>"#,
            i + 1,
            n.title,
            n.body
        )
        .unwrap();
    }
    let sub = format!(
        "v{version} · {side} × {side} grid · {} B of JSON, sent as {} B of CBOR · every box is \
         computed from lc-core's geometry",
        JSON.len(),
        cbor.len()
    );
    let foot = "It must be with a color camera. Grarscale will recognize the image but not successfully decode it. \
                Regenerate this explanatory image with <code>just anatomy</code>.";
    let (pw, ph) = PAGE;
    let html = format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>LC Code anatomy: C3</title>
<style>
  html, body {{ margin: 0; background: #fff; }}
  body {{ width: {pw}px; height: {ph}px; overflow: hidden; color: #1a1a1a;
         font-family: -apple-system, "Helvetica Neue", Helvetica, Arial, sans-serif; }}
  header {{ padding: 30px 48px 0; }}
  h1 {{ margin: 0; font-size: 34px; letter-spacing: -0.01em; }}
  h1 span {{ color: {red}; }}
  .sub {{ margin: 6px 0 0; font-size: 17px; color: #555; }}
  main {{ display: flex; gap: 24px; padding: 0 48px 0 16px; }}
  .fig {{ position: relative; flex: none; width: {fig}px; height: {fig}px; }}
  .fig img {{ position: absolute; left: {PAD}px; top: {PAD}px; width: {canvas}px;
             height: {canvas}px; image-rendering: pixelated; }}
  .fig svg {{ position: absolute; left: 0; top: 0; }}
  ol {{ list-style: none; margin: 0; padding: 40px 0 0; display: flex; flex-direction: column;
        gap: 13px; }}
  li {{ display: flex; gap: 14px; align-items: flex-start; }}
  .badge {{ flex: none; width: 30px; height: 30px; border-radius: 50%; background: {red};
            color: #fff; font-weight: 700; font-size: 16px; display: flex; align-items: center;
            justify-content: center; }}
  h3 {{ margin: 3px 0 3px; font-size: 18px; }}
  p {{ margin: 0; font-size: 15px; line-height: 1.42; color: #333; }}
  footer {{ padding: 4px 48px; font-size: 15px; color: #555; }}
  code {{ font-family: "SF Mono", Menlo, monospace; font-size: 0.92em; background: #f2f2f2;
          padding: 1px 5px; border-radius: 4px; }}
</style></head><body>
<header><h1>LC Code anatomy <span>·</span> C3, finder-less</h1><p class="sub">{sub}</p></header>
<main><div class="fig"><img src="symbol.png" alt="">{svg}</div><ol>{legend}</ol></main>
<footer>{foot}</footer>
</body></html>
"#
    );

    std::fs::write(format!("{dir}/symbol.png"), render_png(&sym, &render)).unwrap();
    std::fs::write(format!("{dir}/anatomy-c3.html"), html).unwrap();
    println!(
        "anatomy: C3 v{version} {side}×{side}, {} notes → {dir}/anatomy-c3.html",
        notes.len()
    );
}
