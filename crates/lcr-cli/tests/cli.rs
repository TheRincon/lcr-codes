//! The `lcr` binary end to end: real arguments, real files, real exit codes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn lcr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lcr"))
        .args(args)
        .output()
        .expect("run lcr")
}

fn ok(args: &[&str]) -> Output {
    let out = lcr(args);
    assert!(
        out.status.success(),
        "lcr {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

/// A fresh directory under the target dir, so tests never touch the source tree.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn json_round_trips_through_png_and_svg() {
    let dir = scratch("json_round_trip");
    let png = dir.join("sym.png");
    let json = r#"{"name":"Fred","loc":"45.00,55.0"}"#;
    let enc = ok(&["encode", "-d", json, "-o", s(&png)]);
    assert!(String::from_utf8_lossy(&enc.stderr).contains("JSON as CBOR"));

    let dec = ok(&["decode", s(&png)]);
    assert_eq!(stdout(&dec).trim(), json);

    let svg = dir.join("sym.svg");
    ok(&["encode", "-d", json, "-o", s(&svg)]);
    assert!(std::fs::read_to_string(&svg).unwrap().contains("<svg"));

    let gdf = dir.join("sym.gdf");
    ok(&["encode", "-d", json, "-o", s(&gdf)]);
    let doc = std::fs::read_to_string(&gdf).unwrap();
    assert!(
        doc.starts_with("%GDF-0.3\n") && doc.ends_with("[end]\n"),
        "{doc}"
    );
}

#[test]
fn text_bytes_and_options() {
    let dir = scratch("kinds");
    let png = dir.join("t.png");
    ok(&["encode", "-d", "42", "-o", s(&png)]);
    assert_eq!(
        stdout(&ok(&["decode", s(&png)])).trim(),
        "42",
        "a bare number is text"
    );

    let input = dir.join("blob.bin");
    std::fs::write(&input, [0u8, 1, 2, 0xff]).unwrap();
    ok(&[
        "encode",
        s(&input),
        "-o",
        s(&png),
        "--profile",
        "c2",
        "--version",
        "5",
        "--ecc",
        "2,1",
    ]);
    let raw = dir.join("blob.out");
    ok(&["decode", s(&png), "-o", s(&raw)]);
    assert_eq!(std::fs::read(&raw).unwrap(), [0u8, 1, 2, 0xff]);

    let report = stdout(&ok(&["decode", "--json", s(&png)]));
    assert!(report.contains(r#""ctype":"opaque""#), "{report}");
    assert!(report.contains(r#""version":5"#), "{report}");
    assert!(report.contains(r#""profile":"C2""#), "{report}");
}

#[test]
fn print_preset_and_preview() {
    let dir = scratch("print");
    let png = dir.join("p.png");
    ok(&["encode", "-d", "paper", "--print", "-o", s(&png)]);
    assert!(stdout(&ok(&["decode", "--json", s(&png)])).contains(r#""profile":"C2""#));

    // No --output: an ANSI preview on stdout.
    let preview = ok(&["encode", "-d", "hello"]);
    assert!(
        stdout(&preview).contains('\u{1b}'),
        "ANSI escapes in the preview"
    );
}

#[test]
fn probe_and_combine() {
    let dir = scratch("probe");
    let png = dir.join("p.png");
    ok(&["encode", "-d", "probe me", "-o", s(&png)]);
    assert!(stdout(&ok(&["probe", s(&png)])).contains("C1"));

    let out = ok(&["decode", "--combine", "-v", s(&png), s(&png), s(&png)]);
    assert_eq!(stdout(&out).trim(), "probe me");
    assert!(String::from_utf8_lossy(&out.stderr).contains("3 frame(s)"));
}

#[test]
fn failures_exit_non_zero_with_a_reason() {
    let dir = scratch("failures");
    let junk = dir.join("junk.png");
    std::fs::write(&junk, b"definitely not an image").unwrap();
    let out = lcr(&["decode", s(&junk)]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a recognised image"));

    let out = lcr(&["encode", "-d", "x", "--module-px", "4"]);
    assert!(!out.status.success(), "below §13.1's 8 px floor");

    let out = lcr(&["encode", "-d", "x", "-o", s(&dir.join("x.gif"))]);
    assert!(String::from_utf8_lossy(&out.stderr).contains(".png, .svg or .gdf"));
}

#[test]
fn capacity_table() {
    let out = stdout(&ok(&["capacity"]));
    assert!(out.lines().count() > 30, "a row per version");
    let one = stdout(&ok(&["capacity", "--version", "4"]));
    assert_eq!(one.lines().count(), 2, "header and one row:\n{one}");
}

#[test]
fn send_and_receive_a_file() {
    let dir = scratch("transfer");
    let file = dir.join("noise.bin");
    let mut s32 = 0x9e37_79b9u32;
    let data: Vec<u8> = (0..4000)
        .map(|_| {
            s32 ^= s32 << 13;
            s32 ^= s32 >> 17;
            s32 ^= s32 << 5;
            s32 as u8
        })
        .collect();
    std::fs::write(&file, &data).unwrap();
    let symbols = dir.join("symbols");
    ok(&["send", s(&file), "-o", s(&symbols), "--version", "16"]);

    let mut images: Vec<PathBuf> = std::fs::read_dir(&symbols)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    images.sort();
    assert!(
        images.len() > 2,
        "4000 bytes of noise needs several symbols"
    );

    // Missing one: the receiver names it, and writes nothing.
    let out_file = dir.join("received.bin");
    let partial: Vec<&str> = images
        .iter()
        .skip(1)
        .filter(|p| !p.ends_with("noise-002.png"))
        .map(|p| s(p))
        .collect();
    let mut args = vec!["receive", "-o", s(&out_file)];
    args.extend(partial.iter().copied());
    let out = lcr(&args);
    assert!(!out.status.success());
    assert!(!out_file.exists());

    // All of them, in reverse: the order does not matter.
    let mut args = vec!["receive", "-o", s(&out_file)];
    args.extend(images.iter().rev().map(|p| s(p)));
    ok(&args);
    assert_eq!(std::fs::read(&out_file).unwrap(), data);

    let out = lcr(&args);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--force"),
        "no silent overwrite"
    );
}

/// One symbol, saved by real third-party encoders in every format `lcr decode` reads.
///
/// The fixtures come from ImageMagick (JPEG, AVIF, both PNG variants), `cwebp` and `cjxl`,
/// all lossy except the PNGs, so this checks the decoders against files this project did
/// not write — and, in CI, that AVIF builds and decodes on x86 as well as ARM.
#[test]
fn every_format_decodes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in [
        "symbol.png",
        "symbol-rgba.png",
        "symbol-indexed.png",
        "symbol.jpg",
        "symbol.webp",
        "symbol.jxl",
        "symbol.avif",
    ] {
        let out = ok(&["decode", s(&dir.join(name))]);
        assert_eq!(stdout(&out).trim(), r#"{"fixture":true}"#, "{name}");
    }
}
