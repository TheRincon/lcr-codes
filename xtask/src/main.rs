//! Offline table generation (§14.1).
//!
//! `cargo run -p xtask -- ldpc` regenerates `crates/lc-core/src/ecc/tables.rs`.
//! §8.2 is explicit that LDPC base graphs are published as static tables and **not**
//! generated at runtime, so this binary is the only place the construction lives, and
//! its output is committed.
//!
//! `cargo run -p xtask -- capacity` prints the §6.1 capacity table computed from the
//! real layout, replacing the spec's hand-estimated figures (open decision #8).
//!
//! `cargo run -p xtask -- palette` searches for the §4 palette constants against the
//! sRGB gamut (open decisions #1 and #2). See `palette.rs`.
//!
//! `cargo run -p xtask -- sweep <log.json>` reduces a real-camera capture log from
//! `web/sweep.html` to §15's success-rate metric. See `sweep.rs`.

mod debruijn;
mod palette;
mod sweep;
mod thresholds;
mod vectors;

use std::collections::VecDeque;
use std::fmt::Write as _;

/// Base-graph column count for the large family.
const NB_LARGE: usize = 60;
/// Base-graph column count for the small family, used when a layer is too short to lift
/// the large one.
const NB_SMALL: usize = 20;
/// Base-graph column count for the tiny family. A v1 `C2` chroma layer is 26 bits, which
/// cannot lift even the 20-column graph at the strongest rates: the parity part alone
/// would be 26 bits, leaving no room for information.
const NB_TINY: usize = 10;
/// The parity structure needs at least three check rows (column 0 sits at rows `0`,
/// `m/2` and `m−1`, which must be distinct).
const MIN_MB: usize = 3;
/// Information-column degree. Regular dv=3 — the standard sweet spot for min-sum.
const DV: usize = 3;
/// Reference lift used when choosing shift values.
const Z0: u16 = 512;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    match arg.as_str() {
        "ldpc" => print!("{}", generate_tables()),
        "capacity" => print!("{}", capacity_table()),
        "palette" => print!("{}", palette::report()),
        "debruijn" => {
            if std::env::args().nth(2).as_deref() == Some("--table") {
                print!("{}", debruijn::table());
            } else {
                print!("{}", debruijn::report());
            }
        }
        "thresholds" => print!("{}", thresholds::report()),
        "vectors" => print!("{}", vectors::report()),
        "sweep" => {
            let path = std::env::args().nth(2).unwrap_or_default();
            if path.is_empty() {
                eprintln!("usage: xtask sweep <log.json>   (exported by web/sweep.html)");
            } else {
                print!("{}", sweep::report(&path));
            }
        }
        other => eprintln!(
            "unknown task {other:?}; expected `ldpc`, `capacity`, `palette`, `thresholds`, \
             `vectors`, `sweep` or `debruijn`"
        ),
    }
}

/// Recompute the §6.1 capacity table from the real layout and codes (open decision #8).
fn capacity_table() -> String {
    use lc_core::capacity::capacity_for_symbol;
    use lc_core::version::{EccLevel, Profile, Version};

    // Two tables, because since D17 there are two layouts and they differ by up to 3.6x.
    // A single table reporting the classic one is how both README.md and the explainer came
    // to quote pre-D17 capacities as if they were current.
    let mut s = String::new();
    for (title, border) in [
        (
            "With the §6.4 ring — the screen default: finder-less (D17), ring payload (D19)",
            true,
        ),
        (
            "Without the ring — print, and anything that may be read in greyscale (classic)",
            false,
        ),
    ] {
        let _ = writeln!(s, "### {title}\n");
        s.push_str("| Version | Side | M | C1 | C2 | C3 |\n|---:|---:|---:|---:|---:|---:|\n");
        for v in Version::all() {
            let _ = write!(s, "| {} | {} |", v.get(), v.side());
            for p in [Profile::M, Profile::C1, Profile::C2, Profile::C3] {
                let cap = capacity_for_symbol(v, p, EccLevel::DEFAULT, border);
                let _ = write!(s, " {} |", cap.body_bytes);
            }
            s.push('\n');
        }
        s.push('\n');
    }
    s
}

/// A base graph: which check rows each information column touches, and with what shift.
struct Base {
    n_b: usize,
    k_b: usize,
    m_b: usize,
    /// `info[j]` = the `DV` `(row, shift)` pairs of information column `j`.
    info: Vec<[(u8, u16); DV]>,
}

fn main_rates() -> [(usize, &'static str); 6] {
    [
        (85, "R085"),
        (75, "R075"),
        (65, "R065"),
        (55, "R055"),
        (45, "R045"),
        (35, "R035"),
    ]
}

/// Parity-column incidence, fixed by the §8.2 encoder structure.
///
/// Column 0 has weight 3 (rows `0`, `m/2`, `m−1`, the middle one shifted by 1) and the
/// rest form a bidiagonal staircase. Summing every row cancels the staircase and leaves
/// `P¹·p₀`, which is what makes systematic encoding a single rotation followed by a
/// forward substitution rather than a matrix inversion.
fn parity_edges(m_b: usize) -> Vec<(usize, usize, u16)> {
    let mut e = vec![(0usize, 0usize, 0u16), (m_b - 1, 0, 0), (m_b / 2, 0, 1)];
    for j in 1..m_b {
        e.push((j, j, 0));
        e.push((j - 1, j, 0));
    }
    e
}

/// Progressive edge growth over the base graph.
///
/// Each information column is attached one edge at a time to whichever check row is
/// currently *furthest* from that column in the partially-built graph, breaking ties by
/// lowest degree. Maximising the distance at attachment time is what maximises the length
/// of the shortest cycle the new edge can close.
fn peg(n_b: usize, k_b: usize) -> Vec<[u8; DV]> {
    let m_b = n_b - k_b;
    // Adjacency including the parity structure, which is fixed before PEG starts so the
    // information edges are placed around it rather than fighting it later.
    let mut row_adj: Vec<Vec<usize>> = vec![Vec::new(); m_b];
    let mut col_adj: Vec<Vec<usize>> = vec![Vec::new(); n_b];
    for (r, c, _) in parity_edges(m_b) {
        row_adj[r].push(c + k_b);
        col_adj[c + k_b].push(r);
    }

    let mut out = Vec::with_capacity(k_b);
    for j in 0..k_b {
        let mut chosen = [0u8; DV];
        for (e, slot) in chosen.iter_mut().enumerate() {
            let pick = if e == 0 {
                (0..m_b).min_by_key(|&r| (row_adj[r].len(), r)).unwrap()
            } else {
                // BFS from column j; prefer unreachable rows, then the deepest ones.
                let mut depth = vec![usize::MAX; m_b];
                let mut q = VecDeque::new();
                for &r in &col_adj[j] {
                    depth[r] = 0;
                    q.push_back(r);
                }
                while let Some(r) = q.pop_front() {
                    for &c in &row_adj[r] {
                        for &r2 in &col_adj[c] {
                            if depth[r2] == usize::MAX {
                                depth[r2] = depth[r] + 1;
                                q.push_back(r2);
                            }
                        }
                    }
                }
                let taken = &col_adj[j];
                (0..m_b)
                    .filter(|r| !taken.contains(r))
                    .max_by_key(|&r| {
                        let d = if depth[r] == usize::MAX {
                            usize::MAX
                        } else {
                            depth[r]
                        };
                        // Deepest first, then lowest degree, then lowest index.
                        (d, usize::MAX - row_adj[r].len(), usize::MAX - r)
                    })
                    .expect("dv must not exceed m_b")
            };
            *slot = pick as u8;
            row_adj[pick].push(j);
            col_adj[j].push(pick);
        }
        out.push(chosen);
    }
    out
}

/// Lift sizes the shift assignment is scored against.
///
/// `shift(Z) = shift₀ mod Z`, so a shift vector that is cycle-free at one lift can still
/// close a four-cycle at another. Scoring against a spread of lifts covering the real
/// operating range — v1 chroma layers lift by 2, v32 luma layers by ~324 — gives a single
/// table that behaves at both ends.
const TEST_Z: [u16; 17] = [
    2, 3, 4, 5, 6, 7, 8, 12, 16, 24, 32, 48, 64, 96, 128, 256, 324,
];

/// Assign shifts greedily, minimising four-cycles across [`TEST_Z`].
fn assign_shifts(n_b: usize, k_b: usize, rows: &[[u8; DV]], seed: u64) -> Vec<[(u8, u16); DV]> {
    let m_b = n_b - k_b;
    // edges[(row, col)] -> shift, for every edge placed so far.
    let mut shift = vec![[0u16; DV]; k_b];
    let mut by_row: Vec<Vec<(usize, usize)>> = vec![Vec::new(); m_b]; // (col, edge index)
    let mut rng = seed;
    let mut next = || {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (rng >> 33) as u32
    };

    for j in 0..k_b {
        for e in 0..DV {
            let r = rows[j][e] as usize;
            let mut best = (u32::MAX, 0u16);
            for _ in 0..256 {
                let cand = (next() % Z0 as u32) as u16;
                let mut penalty = 0u32;
                // Four-cycles closed by (r, j) at shift `cand`: find another row r2 that
                // j already touches, and a column j2 touching both r and r2.
                for e2 in 0..DV {
                    if e2 == e {
                        continue;
                    }
                    let r2 = rows[j][e2] as usize;
                    if r2 == r {
                        penalty += 10_000; // duplicate edge: always bad
                        continue;
                    }
                    for &(j2, e2a) in &by_row[r] {
                        if j2 == j {
                            continue;
                        }
                        for (e2b, &rr) in rows[j2].iter().enumerate() {
                            if rr as usize != r2 {
                                continue;
                            }
                            // Alternating shift sum around the rectangle.
                            let d = cand as i64 - shift[j2][e2a] as i64 + shift[j2][e2b] as i64
                                - shift[j][e2] as i64;
                            if d == 0 {
                                penalty += 1_000;
                            } else {
                                for z in TEST_Z {
                                    if d.rem_euclid(z as i64) == 0 {
                                        penalty += 1;
                                    }
                                }
                            }
                        }
                    }
                }
                if penalty < best.0 {
                    best = (penalty, cand);
                    if penalty == 0 {
                        break;
                    }
                }
            }
            shift[j][e] = best.1;
            by_row[r].push((j, e));
        }
    }

    (0..k_b)
        .map(|j| core::array::from_fn(|e| (rows[j][e], shift[j][e])))
        .collect()
}

fn build(n_b: usize, rate_pct: usize, seed: u64) -> Base {
    let k_b = (n_b * rate_pct / 100).min(n_b - MIN_MB).max(1);
    let rows = peg(n_b, k_b);
    Base {
        n_b,
        k_b,
        m_b: n_b - k_b,
        info: assign_shifts(n_b, k_b, &rows, seed),
    }
}

/// Count residual four-cycles at a given lift, for the report emitted alongside the table.
fn four_cycles(b: &Base, z: u16) -> usize {
    let mut n = 0;
    for j in 0..b.k_b {
        for j2 in (j + 1)..b.k_b {
            for e in 0..DV {
                for e2 in 0..DV {
                    for f in 0..DV {
                        for f2 in 0..DV {
                            if e == e2 || f == f2 {
                                continue;
                            }
                            if b.info[j][e].0 != b.info[j2][f].0
                                || b.info[j][e2].0 != b.info[j2][f2].0
                            {
                                continue;
                            }
                            let d = b.info[j][e].1 as i64 - b.info[j2][f].1 as i64
                                + b.info[j2][f2].1 as i64
                                - b.info[j][e2].1 as i64;
                            if d.rem_euclid(z as i64) == 0 {
                                n += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    n / 2
}

fn generate_tables() -> String {
    let mut s = String::new();
    s.push_str(
        "//! QC-LDPC base graphs (§8.2, spec open decision #4).\n\
         //!\n\
         //! **Generated by `cargo run -p xtask -- ldpc`. Do not edit by hand.**\n\
         //!\n\
         //! Two families of base graph per rate. The large family (`n_b = 60`) is used\n\
         //! wherever the layer is long enough to lift it; the small family (`n_b = 20`)\n\
         //! covers short layers — a v1 `C2` chroma layer is 26 bits, which cannot lift a\n\
         //! 60-column graph to a positive information length at all.\n\
         //!\n\
         //! Rows come from progressive edge growth over the base graph, with the\n\
         //! bidiagonal parity structure fixed first so the information edges are placed\n\
         //! around it. Shifts are chosen greedily to minimise four-cycles across the\n\
         //! spread of lift sizes the format actually uses, since `shift(Z) = shift₀ mod Z`\n\
         //! makes cycle structure lift-dependent.\n\n",
    );
    let _ = writeln!(
        s,
        "/// Information-column degree of every base graph.\npub const DV: usize = {DV};\n"
    );
    let _ = writeln!(
        s,
        "/// One information column: `DV` `(base row, shift)` pairs.\npub type Column = [(u8, u16); DV];\n"
    );
    let _ = writeln!(
        s,
        "/// A quasi-cyclic base graph. Columns `0..k_b` are information, `k_b..n_b` parity.\n\
         #[derive(Clone, Copy, Debug)]\n\
         pub struct BaseGraph {{\n    \
             /// Total base columns.\n    pub n_b: usize,\n    \
             /// Information base columns.\n    pub k_b: usize,\n    \
             /// Check base rows, `n_b − k_b`.\n    pub m_b: usize,\n    \
             /// Incidence and shifts of the information columns.\n    pub info: &'static [Column],\n\
         }}\n"
    );

    let mut report = String::new();
    for (family, n_b) in [("LARGE", NB_LARGE), ("SMALL", NB_SMALL), ("TINY", NB_TINY)] {
        for (i, (pct, name)) in main_rates().iter().enumerate() {
            let b = build(n_b, *pct, 0x5EED_0000 + (i as u64) * 977 + n_b as u64);
            let ident = format!("{name}_{family}");
            let _ = writeln!(
                s,
                "/// Rate {}.{:02} base graph, {} family (`n_b = {}`, `k_b = {}`, `m_b = {}`).",
                pct / 100,
                pct % 100,
                family.to_lowercase(),
                b.n_b,
                b.k_b,
                b.m_b
            );
            let _ = writeln!(s, "pub static {ident}: BaseGraph = BaseGraph {{");
            let _ = writeln!(
                s,
                "    n_b: {},\n    k_b: {},\n    m_b: {},",
                b.n_b, b.k_b, b.m_b
            );
            let _ = writeln!(s, "    info: &{ident}_INFO,");
            let _ = writeln!(s, "}};\n");
            let _ = writeln!(s, "static {ident}_INFO: [Column; {}] = [", b.k_b);
            for col in &b.info {
                let _ = writeln!(
                    s,
                    "    [({}, {}), ({}, {}), ({}, {})],",
                    col[0].0, col[0].1, col[1].0, col[1].1, col[2].0, col[2].1
                );
            }
            s.push_str("];\n\n");

            let c4: Vec<String> = [2u16, 8, 32, 128, 324]
                .iter()
                .map(|&z| format!("Z={z}:{}", four_cycles(&b, z)))
                .collect();
            let _ = writeln!(
                report,
                "//! * `{ident}` residual four-cycles — {}",
                c4.join(", ")
            );
        }
    }

    // Rate lookup tables.
    s.push_str("// Index order in every table below matches `crate::ecc::ldpc::RATES`.\n\n");
    for (family, doc) in [
        ("LARGE", "large family (`n_b = 60`)"),
        ("SMALL", "small family (`n_b = 20`)"),
        ("TINY", "tiny family (`n_b = 10`)"),
    ] {
        let names: Vec<String> = main_rates()
            .iter()
            .map(|(_, n)| format!("{n}_{family}"))
            .collect();
        let _ = writeln!(s, "/// Base graphs by rate index, {doc}.");
        let _ = writeln!(
            s,
            "pub static BY_RATE_{family}: [&BaseGraph; 6] = [&{}];\n",
            names.join(", &")
        );
    }

    s.push_str("\n// Construction report, for the record:\n");
    s.push_str(&report.replace("//! *", "// *"));
    s
}
