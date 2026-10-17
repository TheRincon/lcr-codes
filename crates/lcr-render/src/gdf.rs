//! A symbol as a one-page GDF document.
//!
//! [GDF](https://github.com/TheRincon/gdf) (`%GDF-0.3`) describes pages in readable text.
//! The page here is exactly the canvas — quiet zone and border included, nothing else —
//! drawn with core artifact commands only, so any conforming GDF renderer draws it and no
//! extension is involved:
//!
//! - a `rect` in the commonest colour covering the page;
//! - one `path` per remaining colour, each run a horizontal stroke one module wide,
//!   centred on its row. GDF strokes have butt caps (§13), so a run covers exactly its
//!   cells.
//!
//! **Not an `[image]`.** A 1-sample-per-module PNG stretched to the page would be several
//! times smaller, but GDF leaves image filtering unspecified (§13), and a renderer that
//! smooths the stretch blurs every module edge — measurably costing decode margin. Paths
//! are exact by definition.
//!
//! Lengths are GDF `px` (exactly 0.25 mm), one per device pixel of
//! [`RenderOptions::module_px`]: the screen default of 10 is a 2.5 mm module, print's 8
//! is 2 mm.

use crate::{CellGrid, RenderOptions};
use alloc::string::String;
use core::fmt::Write;
use lcr_core::Symbol;

/// §18.6 caps a GDF path at 65,536 segments. Each run is two (`m`, `h`), so a colour with
/// more runs than this continues in a second `path`.
const RUNS_PER_PATH: usize = 30_000;

/// A length in half-units as a GDF number: `7` is `3.5`, `8` is `4`.
pub(crate) fn half(v: i64) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    if a.is_multiple_of(2) {
        alloc::format!("{sign}{}", a / 2)
    } else {
        alloc::format!("{sign}{}.5", a / 2)
    }
}

/// Render a symbol as a complete, valid `%GDF-0.3` document of one page.
///
/// The page is the canvas and nothing more, so the document is as small as the symbol
/// allows; `language "zxx"` is BCP 47 for "no linguistic content", since the page holds
/// no text.
pub fn render_gdf(symbol: &Symbol, opts: &RenderOptions) -> String {
    let grid = CellGrid::of(symbol, opts);
    let m = opts.module_px.max(1) as i64;
    let page = grid.side as i64 * m;
    let name = |i: usize| alloc::format!("c{i}");

    let mut s = String::with_capacity(8192);
    s.push_str("%GDF-0.3\n\n[meta]\n  title \"LCR Code\"\n  language \"zxx\"\n  unit \"px\"\n");

    // Canonical order: styles by first reference, which is colour order here.
    s.push_str("\n[styles]\n");
    let _ = writeln!(
        s,
        "  paint {} fill:#{:06x}",
        name(0),
        grid.colors[0].to_hex()
    );
    for (i, c) in grid.colors.iter().enumerate().skip(1) {
        let _ = writeln!(
            s,
            "  paint {} stroke:#{:06x} width:{m}",
            name(i),
            c.to_hex()
        );
    }

    let _ = write!(s, "\n[page 1 size:{page}x{page}]\n");
    let _ = writeln!(s, "  rect s:{} box:0,+0,{page},{page}", name(0));
    for i in 1..grid.colors.len() {
        let runs: alloc::vec::Vec<_> = grid.runs(i as u8).collect();
        for chunk in runs.chunks(RUNS_PER_PATH) {
            // In half-units, so a row's centre line stays exact for odd module sizes.
            let (x0, y0, _) = chunk[0];
            let (ax, ay) = (2 * x0 as i64 * m, (2 * y0 as i64 + 1) * m);
            let _ = write!(s, "  path s:{} at:{},+{} \"", name(i), half(ax), half(ay));
            let (mut cx, mut cy) = (ax, ay);
            for &(x, y, len) in chunk {
                let (px, py) = (2 * x as i64 * m, (2 * y as i64 + 1) * m);
                let _ = write!(s, "m{} {}h{}", half(px - cx), half(py - cy), len as i64 * m);
                cx = px + 2 * len as i64 * m;
                cy = py;
            }
            s.push_str("\"\n");
        }
    }
    s.push_str("\n[end]\n");
    s
}
