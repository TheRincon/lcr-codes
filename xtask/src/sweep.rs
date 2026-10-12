//! Reduce a capture-sweep log to §15's metric.
//!
//! `web/sweep.html` records one line per decode attempt against the geometry it was
//! attempted at. This turns that into **success rate vs. degradation magnitude, reported
//! per profile** — which is what §15 asks for and what a pass/fail grid cannot give.
//!
//! The two magnitudes a real capture sweeps are distance and angle, and the log carries
//! both in intrinsics-free form:
//!
//! * **Captured module pitch** — sensor pixels per module. This is distance, in the unit
//!   decoding actually depends on. §13.1's ≥ 8 floor is stated in *display* pixels per
//!   module, and the capture-side figure is the one open decision #14 needs.
//! * **Obliquity** — coarsest module pitch over finest, across the symbol. 1.0 face-on,
//!   climbing with viewing angle.
//!
//! Parsing uses `lcr_core::json`, which is the project's own parser and the one the
//! encoder authors through (§9.4), so the tool cannot drift from the crate.

use lcr_core::cbor::Value;
use lcr_core::json;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Trials below this in a bin are reported but not treated as evidence.
const THIN: usize = 10;

pub fn report(path: &str) -> String {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return format!("cannot read {path}: {e}\n"),
    };
    let root = match json::parse(&text) {
        Ok(v) => v,
        Err(e) => return format!("{path} is not valid JSON: {e:?}\n"),
    };
    let Some(attempts) = get(&root, "attempts").and_then(as_array) else {
        return format!("{path} has no `attempts` array\n");
    };

    let trials: Vec<Trial> = attempts.iter().filter_map(Trial::read).collect();
    if trials.is_empty() {
        return format!("{path} recorded no usable trials\n");
    }

    let mut s = String::new();
    let _ = writeln!(s, "# Capture sweep — {path}\n");
    session(&mut s, &root, &trials);
    for run in runs(&trials) {
        let in_run: Vec<&Trial> = trials.iter().filter(|t| t.run == run).collect();
        let _ = writeln!(s, "\n## Run `{run}` — {} trials\n", in_run.len());
        by_profile(&mut s, &in_run);
    }
    s
}

// ---------------------------------------------------------------- model

struct Trial {
    run: String,
    profile: String,
    decoded: bool,
    found: bool,
    pitch: Option<f64>,
    obliquity: Option<f64>,
    mode: String,
}

impl Trial {
    fn read(v: &Value) -> Option<Trial> {
        let outcome = get(v, "outcome").and_then(as_str)?;
        Some(Trial {
            run: get(v, "run")
                .and_then(as_str)
                .unwrap_or("unlabelled")
                .to_string(),
            // A frame where nothing was found has no format region and so no profile.
            profile: get(v, "profile")
                .and_then(as_str)
                .unwrap_or("?")
                .to_string(),
            decoded: outcome == "decoded",
            // `found` is absent on a success, where locating the symbol is implied.
            found: get(v, "found").and_then(as_bool).unwrap_or(true),
            pitch: get(v, "pitchMin").and_then(as_f64),
            obliquity: get(v, "obliquity").and_then(as_f64),
            mode: get(v, "mode")
                .and_then(as_str)
                .unwrap_or("single")
                .to_string(),
        })
    }
}

fn runs(trials: &[Trial]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for t in trials {
        if !seen.contains(&t.run) {
            seen.push(t.run.clone());
        }
    }
    seen
}

// ---------------------------------------------------------------- reporting

fn session(s: &mut String, root: &Value, trials: &[Trial]) {
    let field = |k: &str| {
        get(root, "session")
            .and_then(|v| get(v, k))
            .and_then(as_str)
            .filter(|t| !t.is_empty())
            .unwrap_or("—")
            .to_string()
    };
    let _ = writeln!(s, "| | |\n|---|---|");
    for (label, key) in [
        ("Started", "started"),
        ("Operator", "operator"),
        ("Camera", "phone"),
        ("Panel", "panel"),
        ("Ambient", "ambient"),
    ] {
        let _ = writeln!(s, "| {label} | {} |", field(key));
    }
    let px = get(root, "display")
        .and_then(|v| get(v, "modulePx"))
        .and_then(as_f64);
    let _ = writeln!(
        s,
        "| Display px/module | {} |",
        px.map_or("—".into(), |p| format!("{p:.0}"))
    );
    let accumulated = trials.iter().filter(|t| t.mode != "single").count();
    let _ = writeln!(
        s,
        "| Trials | {} ({} single-frame, {accumulated} accumulated) |",
        trials.len(),
        trials.len() - accumulated,
    );
    if accumulated > 0 && accumulated < trials.len() {
        let _ = writeln!(
            s,
            "\n**Mixed modes in one file.** Accumulated trials measure how long the \
             operator lingered as much as where they stood; read the two apart before \
             quoting a rate."
        );
    }
}

fn by_profile(s: &mut String, trials: &[&Trial]) {
    let mut profiles: Vec<String> = Vec::new();
    for t in trials {
        if !profiles.contains(&t.profile) {
            profiles.push(t.profile.clone());
        }
    }
    // A "?" profile means the symbol was never located, so it belongs to no profile's
    // envelope. Reported once, separately, rather than smeared across the others.
    profiles.retain(|p| p != "?");
    profiles.sort();

    for p in &profiles {
        let rows: Vec<&&Trial> = trials.iter().filter(|t| &t.profile == p).collect();
        let _ = writeln!(
            s,
            "### Profile `{}` — {} trials\n",
            p.to_uppercase(),
            rows.len()
        );
        histogram(
            s,
            "Captured px/module",
            &rows,
            |t| t.pitch,
            |v| format!("{:.0}–{:.0}", v.floor(), v.floor() + 1.0),
            1.0,
        );
        s.push('\n');
        histogram(
            s,
            "Obliquity",
            &rows,
            |t| t.obliquity,
            |v| {
                let lo = (v / 0.1).floor() * 0.1;
                format!("{lo:.1}–{:.1}", lo + 0.1)
            },
            0.1,
        );
        s.push('\n');
        face_on_split(s, &rows);
        s.push('\n');
        working_range(s, &rows);
    }

    let lost = trials.iter().filter(|t| !t.found).count();
    if lost > 0 {
        let _ = writeln!(
            s,
            "\n**{lost} of {} frames located no symbol at all** ({:.0}%) — a different \
             limit from a symbol found and not decoded, and the one that says the operator \
             was out of range rather than out of margin.",
            trials.len(),
            100.0 * lost as f64 / trials.len() as f64,
        );
    }
}

/// Success rate face-on against tilted, at matched pitch.
///
/// Broken out because sweeps 1 and 2 found this to be **the dominant effect**, larger than
/// distance over the whole usable range: at 7–8 captured px/module, face-on decoded 4% and
/// a tilt past 1.35 obliquity decoded 93%, in the same minutes at the same range. §13.2
/// already recommends 10–20° off-axis so the phone's own reflection lands away from the
/// symbol; the measurement says it is worth more than anything else the operator controls.
///
/// Matched by pitch band so the comparison cannot be a distance effect in disguise — which
/// is exactly what it looks like until you control for it.
fn face_on_split(s: &mut String, trials: &[&&Trial]) {
    let mut rows: Vec<String> = Vec::new();
    for p in 5..15 {
        let band: Vec<&&&Trial> = trials
            .iter()
            .filter(|t| t.pitch.is_some_and(|v| v >= p as f64 && v < (p + 1) as f64))
            .collect();
        let rate = |sel: &dyn Fn(f64) -> bool| -> Option<(f64, usize)> {
            let g: Vec<_> = band
                .iter()
                .filter(|t| t.obliquity.is_some_and(sel))
                .collect();
            (g.len() >= 5).then(|| {
                let ok = g.iter().filter(|t| t.decoded).count();
                (100.0 * ok as f64 / g.len() as f64, g.len())
            })
        };
        let flat = rate(&|o| o < 1.15);
        let tilt = rate(&|o| o >= 1.35);
        if let (Some((fr, fon)), Some((tr, tn))) = (flat, tilt) {
            rows.push(format!(
                "| {p}–{} | {fr:.0}% ({fon}) | {tr:.0}% ({tn}) | **{:+.0} pts** |",
                p + 1,
                tr - fr,
            ));
        }
    }
    if rows.is_empty() {
        let _ = writeln!(
            s,
            "*Not enough matched-pitch trials to compare face-on against tilted.*"
        );
        return;
    }
    let _ = writeln!(
        s,
        "**Face-on against tilted, at matched pitch** — §13.2's off-axis advice, measured:\n"
    );
    let _ = writeln!(
        s,
        "| px/module | face-on (<1.15) | tilted (≥1.35) | difference |\n|---|---:|---:|---:|"
    );
    for r in rows {
        let _ = writeln!(s, "{r}");
    }
}

/// Success rate against a binned magnitude — §15's primary regression metric.
fn histogram<F, L>(s: &mut String, axis: &str, trials: &[&&Trial], key: F, label: L, width: f64)
where
    F: Fn(&Trial) -> Option<f64>,
    L: Fn(f64) -> String,
{
    let mut bins: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    for t in trials {
        let Some(v) = key(t) else { continue };
        let e = bins.entry((v / width).floor() as i64).or_insert((0, 0));
        e.0 += 1;
        if t.decoded {
            e.1 += 1;
        }
    }
    if bins.is_empty() {
        let _ = writeln!(s, "*No {} recorded.*", axis.to_lowercase());
        return;
    }
    let _ = writeln!(
        s,
        "| {axis} | trials | decoded | rate | |\n|---|---:|---:|---:|---|"
    );
    for (k, (n, ok)) in &bins {
        let rate = 100.0 * *ok as f64 / *n as f64;
        let bar = "#".repeat((rate / 5.0).round() as usize);
        let flag = if *n < THIN { " *(thin)*" } else { "" };
        let _ = writeln!(
            s,
            "| {} | {n} | {ok} | {rate:.0}%{flag} | `{bar}` |",
            label(*k as f64 * width),
        );
    }
}

/// The number the sweep exists to produce: where it stopped working.
///
/// Reported as the finest pitch that still cleared a threshold on a bin with enough
/// trials to mean anything, rather than as the single best scan — one lucky decode at the
/// limit is not a working range.
fn working_range(s: &mut String, trials: &[&&Trial]) {
    let mut bins: BTreeMap<i64, (usize, usize)> = BTreeMap::new();
    for t in trials {
        let Some(v) = t.pitch else { continue };
        let e = bins.entry(v.floor() as i64).or_insert((0, 0));
        e.0 += 1;
        if t.decoded {
            e.1 += 1;
        }
    }
    let solid = |threshold: f64| -> Option<i64> {
        bins.iter()
            .filter(|(_, (n, ok))| *n >= THIN && 100.0 * *ok as f64 / *n as f64 >= threshold)
            .map(|(k, _)| *k)
            .min()
    };
    match (solid(95.0), solid(50.0)) {
        (Some(reliable), Some(marginal)) => {
            let _ = writeln!(
                s,
                "**Working range:** ≥ 95% from **{reliable} px/module** up; ≥ 50% from \
                 **{marginal} px/module** up.",
            );
        }
        (None, Some(marginal)) => {
            let _ = writeln!(
                s,
                "**Working range:** no bin reached 95% with ≥ {THIN} trials; ≥ 50% from \
                 **{marginal} px/module** up.",
            );
        }
        _ => {
            let _ = writeln!(
                s,
                "**Working range:** not established — no bin has ≥ {THIN} trials at ≥ 50%. \
                 Sweep more slowly, or longer.",
            );
        }
    }
}

// ---------------------------------------------------------------- json helpers

fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Map(entries) => entries
            .iter()
            .find(|(k, _)| matches!(k, Value::Text(t) if t == key))
            .map(|(_, v)| v),
        _ => None,
    }
}

fn as_array(v: &Value) -> Option<&Vec<Value>> {
    match v {
        Value::Array(a) => Some(a),
        _ => None,
    }
}

fn as_str(v: &Value) -> Option<&str> {
    match v {
        Value::Text(t) => Some(t),
        _ => None,
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Uint(u) => Some(*u as f64),
        // `Nint` stores `n` for the encoded value `−1 − n`.
        Value::Nint(n) => Some(-1.0 - *n as f64),
        _ => None,
    }
}
