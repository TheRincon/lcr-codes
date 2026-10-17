//! `lcr` — LCR Code from the command line.
//!
//! A thin layer over the library crates: every command is argument parsing, a call into
//! `lcr-encode`, `lcr-decode` or `lcr-cft`, and output formatting. Nothing here is logic
//! the libraries lack, so anything the CLI can do a Rust or C host can do the same way.
//!
//! Image input goes through [`load`], which reads PNG, JPEG, WebP, JPEG XL and AVIF by
//! content rather than by extension.

mod load;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use lcr_core::cbor::Value;
use lcr_core::{ContentType, EccLevel, Packet, Profile, Version};
use lcr_decode::image::{ImageView, PixelFormat};
use lcr_decode::{Decoder, FrameAccumulator};
use lcr_render::RenderOptions;

#[derive(Parser)]
#[command(
    name = "lcr",
    version,
    about = "Encode, decode and transfer files as LCR Code symbols"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Encode data as a symbol: PNG, SVG, or a preview in the terminal.
    Encode(EncodeArgs),
    /// Read the payload from one or more images.
    Decode(DecodeArgs),
    /// Find a symbol and report its geometry, without decoding the payload.
    Probe(ProbeArgs),
    /// Show how many bytes a symbol holds, per version and profile.
    Capacity(CapacityArgs),
    /// Split a file into a sequence of symbols (CBOR File Transfer).
    Send(SendArgs),
    /// Reassemble a file from images of the symbols `lcr send` wrote.
    Receive(ReceiveArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Encode(a) => encode(a),
        Command::Decode(a) => decode(a),
        Command::Probe(a) => probe(a),
        Command::Capacity(a) => capacity(a),
        Command::Send(a) => send(a),
        Command::Receive(a) => receive(a),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("lcr: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

type Result<T = ()> = std::result::Result<T, String>;

/// `print!` and `println!` for standard output, minus the panic when the reader goes away.
///
/// `lcr capacity | head` closes the pipe after ten lines, and std's macros turn the
/// resulting `BrokenPipe` into a panic with a backtrace hint. A command-line tool should
/// treat a reader that has seen enough as success, so this exits quietly instead.
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if let Err(e) = write!(std::io::stdout(), $($arg)*) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
            return Err(format!("writing output: {e}"));
        }
    }};
}

macro_rules! outln {
    ($($arg:tt)*) => {{
        out!($($arg)*);
        out!("\n");
    }};
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---------------------------------------------------------------- shared options

#[derive(Clone, Copy, ValueEnum)]
enum ProfileArg {
    /// Monochrome, 1 bit/module. Survives a capture too poor to trust colour.
    Mono,
    /// 4 colours, co-sited chroma. The screen default.
    C1,
    /// 4 colours, 2×2 chroma cells. The print default: more blur tolerance, less capacity.
    C2,
    /// 8 colours. The densest and the least robust — opt in knowingly.
    C3,
}

impl From<ProfileArg> for Profile {
    fn from(p: ProfileArg) -> Profile {
        match p {
            ProfileArg::Mono => Profile::M,
            ProfileArg::C1 => Profile::C1,
            ProfileArg::C2 => Profile::C2,
            ProfileArg::C3 => Profile::C3,
        }
    }
}

fn profile_name(p: Profile) -> &'static str {
    match p {
        Profile::M => "mono",
        Profile::C1 => "C1",
        Profile::C2 => "C2",
        Profile::C3 => "C3",
    }
}

/// `L` or `L,C`: error-correction levels 0–3 for the luma and chroma layers.
fn parse_ecc(s: &str) -> Result<EccLevel> {
    let level = |t: &str| {
        t.trim()
            .parse::<u8>()
            .map_err(|_| format!("`{t}` is not a level 0–3"))
    };
    let (l, c) = match s.split_once(',') {
        Some((l, c)) => (level(l)?, level(c)?),
        None => {
            let l = level(s)?;
            (l, l)
        }
    };
    EccLevel::new(l, c).map_err(err)
}

fn parse_version(s: &str) -> Result<Version> {
    let v = s
        .parse::<u8>()
        .map_err(|_| format!("`{s}` is not a version 1–64"))?;
    Version::new(v).map_err(err)
}

/// Options shared by everything that writes symbols.
#[derive(Args)]
struct SymbolArgs {
    /// Colour profile. Default C1, or C2 with --print.
    #[arg(long, value_enum)]
    profile: Option<ProfileArg>,
    /// Symbol version 1–64. Default: the smallest that fits (versions above 32 need the
    /// reference border).
    #[arg(long, value_parser = parse_version)]
    version: Option<Version>,
    /// Error correction, `L` for both layers or `L,C` for luma and chroma separately (0–3).
    #[arg(long, value_parser = parse_ecc, default_value = "1")]
    ecc: EccLevel,
    /// Omit the §6.4 reference border. It costs no payload capacity and helps under glare,
    /// so leave it on for screens.
    #[arg(long)]
    no_border: bool,
    /// Settings for paper: profile C2, no border, 8 px modules.
    #[arg(long)]
    print: bool,
    /// Pixels per module, at least 8 (§13.1). Default 10, or 8 with --print.
    #[arg(long, value_parser = clap::value_parser!(u32).range(8..))]
    module_px: Option<u32>,
}

impl SymbolArgs {
    fn encode_options(&self) -> lcr_encode::EncodeOptions {
        let base = if self.print {
            lcr_encode::EncodeOptions::print()
        } else {
            lcr_encode::EncodeOptions::screen()
        };
        lcr_encode::EncodeOptions {
            profile: self.profile.map_or(base.profile, Profile::from),
            version: self.version,
            ecc: self.ecc,
            reference_border: base.reference_border && !self.no_border,
            ..base
        }
    }

    fn render_options(&self) -> RenderOptions {
        let base = if self.print {
            RenderOptions::print()
        } else {
            RenderOptions::screen()
        };
        RenderOptions {
            module_px: self.module_px.unwrap_or(base.module_px),
            ..base
        }
    }
}

fn describe(sym: &lcr_core::Symbol) -> String {
    let side = sym.format.version.side();
    format!(
        "v{} ({side}×{side} modules), {}",
        sym.format.version.get(),
        profile_name(sym.format.profile)
    )
}

// ---------------------------------------------------------------- encode

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum Kind {
    /// JSON when the input looks like a JSON object or array, else text when it is UTF-8,
    /// else bytes.
    Auto,
    /// JSON, sent as CBOR (§9.4) and printed back as JSON by `lcr decode`.
    Json,
    /// UTF-8 text.
    Text,
    /// Opaque bytes.
    Bytes,
}

#[derive(Args)]
struct EncodeArgs {
    /// File to encode, or `-` for standard input.
    #[arg(required_unless_present = "data")]
    input: Option<PathBuf>,
    /// Encode this string instead of a file.
    #[arg(short, long, conflicts_with = "input")]
    data: Option<String>,
    /// How to interpret the input.
    #[arg(long = "as", value_enum, default_value_t = Kind::Auto)]
    kind: Kind,
    /// Write the symbol here: `.png`, `.svg`, or `.gdf` (a one-page GDF document). Without
    /// it, a preview prints to the terminal.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Compression: auto tries each and keeps the smallest (§9.3).
    #[arg(long, value_enum, default_value_t = CompressionArg::Auto)]
    compression: CompressionArg,
    /// Data mask 0–7. Default: the §8.5 penalty search picks one.
    #[arg(long)]
    mask: Option<u8>,
    #[command(flatten)]
    symbol: SymbolArgs,
}

#[derive(Clone, Copy, ValueEnum)]
enum CompressionArg {
    Auto,
    None,
    Deflate,
}

fn read_input(a: &EncodeArgs) -> Result<Vec<u8>> {
    if let Some(d) = &a.data {
        return Ok(d.clone().into_bytes());
    }
    match a.input.as_deref() {
        Some(p) if p != Path::new("-") => {
            std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))
        }
        _ => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf).map_err(err)?;
            Ok(buf)
        }
    }
}

fn resolve_kind(kind: Kind, data: &[u8]) -> Kind {
    if kind != Kind::Auto {
        return kind;
    }
    match std::str::from_utf8(data) {
        // Only objects and arrays. `42` and `"hi"` are valid JSON too, but someone typing
        // `lcr encode -d 42` means the text, not a CBOR integer.
        Ok(s)
            if matches!(s.trim_start().as_bytes().first(), Some(b'{' | b'['))
                && lcr_core::json::parse(s).is_ok() =>
        {
            Kind::Json
        }
        Ok(_) => Kind::Text,
        Err(_) => Kind::Bytes,
    }
}

fn encode(a: EncodeArgs) -> Result {
    let data = read_input(&a)?;
    let opts = lcr_encode::EncodeOptions {
        compression: match a.compression {
            CompressionArg::Auto => None,
            CompressionArg::None => Some(lcr_core::Compression::None),
            CompressionArg::Deflate => Some(lcr_core::Compression::Deflate),
        },
        mask: a.mask,
        ..a.symbol.encode_options()
    };
    let kind = resolve_kind(a.kind, &data);
    let utf8 = || std::str::from_utf8(&data).map_err(|e| format!("input is not UTF-8: {e}"));
    let sym = match kind {
        Kind::Json => lcr_encode::encode_json(utf8()?, &opts),
        Kind::Text => lcr_encode::encode_text(utf8()?, &opts),
        Kind::Bytes | Kind::Auto => lcr_encode::encode_bytes(&data, &opts),
    }
    .map_err(err)?;

    let ropts = a.symbol.render_options();
    let what = match kind {
        Kind::Json => "JSON as CBOR",
        Kind::Text => "text",
        Kind::Bytes | Kind::Auto => "bytes",
    };
    let Some(out) = a.output else {
        out!("{}", lcr_render::render_ansi(&sym));
        eprintln!("{} bytes of {what}: {}", data.len(), describe(&sym));
        return Ok(());
    };
    let ext = out
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let bytes = match ext.as_deref() {
        Some("svg") => lcr_render::render_svg(&sym, &ropts).into_bytes(),
        Some("gdf") => lcr_render::render_gdf(&sym, &ropts).into_bytes(),
        Some("png") => lcr_render::render_png(&sym, &ropts),
        _ => {
            return Err(format!(
                "{}: the output must end in .png, .svg or .gdf",
                out.display()
            ));
        }
    };
    std::fs::write(&out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!(
        "{} bytes of {what}: {} → {}",
        data.len(),
        describe(&sym),
        out.display()
    );
    Ok(())
}

// ---------------------------------------------------------------- decode

#[derive(Args)]
struct DecodeArgs {
    /// Images to read: PNG, JPEG, WebP, JPEG XL or AVIF.
    #[arg(required = true)]
    images: Vec<PathBuf>,
    /// Treat the images as frames of one symbol and combine them before decoding (§12.4).
    /// Glare and banding move between frames, so this recovers captures no single frame
    /// can.
    #[arg(long)]
    combine: bool,
    /// Write the payload's raw bytes to this file instead of printing it.
    #[arg(short, long, conflicts_with = "json")]
    output: Option<PathBuf>,
    /// Print a JSON report per image — packet, content type, payload and diagnostics —
    /// instead of the payload.
    #[arg(long)]
    json: bool,
    /// Print decode diagnostics to stderr.
    #[arg(short, long)]
    verbose: bool,
}

fn view(f: &load::Frame) -> Result<ImageView<'_>> {
    let stride = f.width * f.format.channels() as u32;
    ImageView::new(&f.pixels, f.width, f.height, stride, f.format).map_err(err)
}

/// Decode one loaded image, explaining the one failure a user can act on: a greyscale
/// image of a colour symbol. Only `mono` symbols survive a grey capture whole; a ringed
/// colour symbol fails closed by design, because its ring is coloured.
fn decode_frame(decoder: &Decoder, f: &load::Frame) -> Result<lcr_decode::DecodeOutput> {
    decoder.decode(&view(f)?).map_err(|e| {
        if f.format == PixelFormat::Gray8 {
            format!("{e} (the image is greyscale, which only `--profile mono` symbols survive)")
        } else {
            e.to_string()
        }
    })
}

/// The payload as a person should see it, or why there is none.
fn payload(packet: &Packet) -> Result<(ContentType, &[u8])> {
    match packet {
        Packet::Unsigned { body, ctype } => Ok((*ctype, body)),
        Packet::Verified { body, .. } => Ok((ContentType::Cbor, body)),
        Packet::Unverified { .. } => {
            Err("signed packet (COSE_Sign1) that this CLI cannot verify; \
                                         its content is withheld unverified (§10.8)"
                .into())
        }
        Packet::Partial { .. } => Err("only a partial luma prefix was recovered, which is a \
                                      diagnostic and not content (§8.1)"
            .into()),
        _ => Err("unrecognised packet kind".into()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Render a payload for the terminal: CBOR as JSON, text as itself, anything else as hex.
fn printable(ctype: ContentType, body: &[u8]) -> String {
    match ctype {
        ContentType::Cbor => match Value::from_slice(body) {
            Ok(v) => lcr_core::json::to_json(&v),
            Err(_) => hex(body),
        },
        ContentType::Text | ContentType::Json => match std::str::from_utf8(body) {
            Ok(s) => s.to_owned(),
            Err(_) => hex(body),
        },
        _ => hex(body),
    }
}

fn ctype_name(c: ContentType) -> &'static str {
    match c {
        ContentType::Opaque => "opaque",
        ContentType::Text => "text",
        ContentType::Json => "json",
        ContentType::Cbor => "cbor",
        ContentType::CoseSign1 => "cose-sign1",
        ContentType::Multipart => "multipart",
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn diagnostics_value(d: &lcr_decode::Diagnostics) -> Value {
    let f = |x: f32| Value::Float(x as f64);
    let u = |x: u64| Value::Uint(x);
    Value::Map(vec![
        (text("version"), u(d.format.version.get().into())),
        (text("profile"), text(profile_name(d.format.profile))),
        (text("snr_db"), f(d.estimated_snr_db)),
        (text("chroma_recovered"), Value::Bool(d.chroma_recovered)),
        (text("frames_combined"), u(d.frames_combined.into())),
        (text("luma_iterations"), u(d.luma_iterations.into())),
        (text("chroma_iterations"), u(d.chroma_iterations.into())),
        (text("illum_residual_rms"), f(d.illum_residual_rms)),
        (text("references_rejected"), u(d.references_rejected.into())),
        (text("format_margin"), f(d.format_margin)),
        (text("alignment_found"), Value::Bool(d.alignment_found)),
        (text("capture_pitch_min"), f(d.capture_pitch_min)),
        (text("capture_pitch_max"), f(d.capture_pitch_max)),
    ])
}

fn report(source: &str, out: &std::result::Result<lcr_decode::DecodeOutput, String>) -> Value {
    let mut m = vec![(text("source"), text(source))];
    match out {
        Ok(o) => {
            match payload(&o.packet) {
                Ok((ctype, body)) => {
                    m.push((text("ok"), Value::Bool(true)));
                    m.push((text("ctype"), text(ctype_name(ctype))));
                    // CBOR is embedded as the value it is, not as a string of JSON.
                    let value = match (ctype, Value::from_slice(body)) {
                        (ContentType::Cbor, Ok(v)) => v,
                        _ => match (ctype, std::str::from_utf8(body)) {
                            (ContentType::Text | ContentType::Json, Ok(s)) => text(s),
                            _ => text(&hex(body)),
                        },
                    };
                    m.push((text("payload"), value));
                }
                Err(e) => {
                    m.push((text("ok"), Value::Bool(false)));
                    m.push((text("error"), text(&e)));
                }
            }
            m.push((text("diagnostics"), diagnostics_value(&o.diagnostics)));
        }
        Err(e) => {
            m.push((text("ok"), Value::Bool(false)));
            m.push((text("error"), text(e)));
        }
    }
    Value::Map(m)
}

fn decode(a: DecodeArgs) -> Result {
    let decoder = Decoder::new();
    let mut results: Vec<(
        String,
        std::result::Result<lcr_decode::DecodeOutput, String>,
    )> = Vec::new();
    if a.combine {
        let mut acc = FrameAccumulator::new();
        for p in &a.images {
            let frame = load::load(p).map_err(|e| format!("{}: {e}", p.display()))?;
            if let Err(e) = acc.push(&view(&frame)?) {
                eprintln!("lcr: {}: frame skipped: {e}", p.display());
            }
        }
        let out = if acc.frames() == 0 {
            Err("no frame contained a symbol".to_owned())
        } else {
            acc.try_decode(&decoder).map_err(err)
        };
        results.push((format!("{} frames", acc.frames()), out));
    } else {
        for p in &a.images {
            let out = load::load(p).and_then(|f| decode_frame(&decoder, &f));
            results.push((p.display().to_string(), out));
        }
    }

    let many = results.len() > 1;
    let mut failed = 0;
    for (source, out) in &results {
        if a.json {
            outln!("{}", lcr_core::json::to_json(&report(source, out)));
            failed += report_failed(out) as usize;
            continue;
        }
        let o = match out {
            Ok(o) => o,
            Err(e) => {
                eprintln!("lcr: {source}: {e}");
                failed += 1;
                continue;
            }
        };
        if a.verbose {
            let d = &o.diagnostics;
            eprintln!(
                "{source}: v{} {}, SNR {:.1} dB, chroma {}, {} frame(s), pitch {:.1}–{:.1} px",
                d.format.version.get(),
                profile_name(d.format.profile),
                d.estimated_snr_db,
                if d.chroma_recovered {
                    "recovered"
                } else {
                    "lost"
                },
                d.frames_combined,
                d.capture_pitch_min,
                d.capture_pitch_max,
            );
        }
        let (ctype, body) = match payload(&o.packet) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("lcr: {source}: {e}");
                failed += 1;
                continue;
            }
        };
        if let Some(path) = &a.output {
            if many {
                return Err("--output takes one payload; pass one image, or use --combine".into());
            }
            std::fs::write(path, body).map_err(|e| format!("{}: {e}", path.display()))?;
            eprintln!(
                "{} bytes ({}) → {}",
                body.len(),
                ctype_name(ctype),
                path.display()
            );
        } else if many {
            outln!("{source}: {}", printable(ctype, body));
        } else {
            outln!("{}", printable(ctype, body));
        }
    }
    match failed {
        0 => Ok(()),
        n if many => Err(format!("{n} of {} failed", results.len())),
        // The single failure was already reported above; exit non-zero without repeating it.
        _ => Err(String::new()),
    }
}

fn report_failed(out: &std::result::Result<lcr_decode::DecodeOutput, String>) -> bool {
    out.as_ref().map_or(true, |o| payload(&o.packet).is_err())
}

// ---------------------------------------------------------------- probe

#[derive(Args)]
struct ProbeArgs {
    /// Images to examine.
    #[arg(required = true)]
    images: Vec<PathBuf>,
}

fn probe(a: ProbeArgs) -> Result {
    let decoder = Decoder::new();
    let mut failed = 0;
    for p in &a.images {
        let found = load::load(p).and_then(|f| decoder.probe(&view(&f)?).map_err(err));
        match found {
            Ok(r) => outln!(
                "{}: v{} {}, format margin {:.2}, {}, pitch {:.1}–{:.1} px, illumination residual {:.4}, {} reference(s) rejected",
                p.display(),
                r.format.version.get(),
                profile_name(r.format.profile),
                r.format_margin,
                if r.alignment_found {
                    "homography"
                } else {
                    "affine fallback"
                },
                r.capture_pitch_min,
                r.capture_pitch_max,
                r.illum_residual_rms,
                r.references_rejected,
            ),
            Err(e) => {
                outln!("{}: no symbol found ({e})", p.display());
                failed += 1;
            }
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!(
            "{failed} of {} had no readable symbol",
            a.images.len()
        ))
    }
}

// ---------------------------------------------------------------- capacity

#[derive(Args)]
struct CapacityArgs {
    /// Just this version. Default: every version.
    #[arg(long, value_parser = parse_version)]
    version: Option<Version>,
    /// Error correction, `L` or `L,C` (0–3).
    #[arg(long, value_parser = parse_ecc, default_value = "1")]
    ecc: EccLevel,
    /// Capacity without the reference border, as for print.
    #[arg(long)]
    no_border: bool,
}

fn capacity(a: CapacityArgs) -> Result {
    const PROFILES: [Profile; 4] = [Profile::M, Profile::C1, Profile::C2, Profile::C3];
    let border = !a.no_border;
    outln!(
        "{:>7} {:>9} {:>7} {:>7} {:>7} {:>7}",
        "version",
        "modules",
        "mono",
        "C1",
        "C2",
        "C3"
    );
    let versions: Vec<Version> = match a.version {
        Some(v) => vec![v],
        None => Version::all().collect(),
    };
    for v in versions {
        let side = v.side();
        let mut row = format!("{:>7} {:>9}", v.get(), format!("{side}×{side}"));
        let mut any = false;
        for p in PROFILES {
            // Classic (finder-bearing) symbols stop at v32: their format word has five bits
            // for the version. Only the ringed layout goes higher.
            let ceiling = if lcr_core::layout::Layout::is_finderless(p, border) {
                Version::MAX
            } else {
                Version::MAX_CLASSIC
            };
            if v > ceiling {
                row.push_str(&format!(" {:>7}", "—"));
                continue;
            }
            let cap = lcr_encode::capacity_for_symbol(v, p, a.ecc, border);
            row.push_str(&format!(" {:>7}", cap.body_bytes));
            any = true;
        }
        if any {
            outln!("{row}");
        }
    }
    eprintln!(
        "bytes of framed payload; ECC {}, border {}",
        ecc_name(a.ecc),
        if border { "on" } else { "off" }
    );
    Ok(())
}

fn ecc_name(e: EccLevel) -> String {
    if e.luma == e.chroma {
        e.luma.to_string()
    } else {
        format!("{},{}", e.luma, e.chroma)
    }
}

// ---------------------------------------------------------------- send / receive

#[derive(Args)]
struct SendArgs {
    /// The file to send.
    file: PathBuf,
    /// Directory to write the symbols into, created if needed.
    #[arg(short, long)]
    out_dir: PathBuf,
    /// Name the receiver will save the file as. Default: the file's own name.
    #[arg(long)]
    name: Option<String>,
    #[command(flatten)]
    symbol: SymbolArgs,
}

/// Eight bytes that will not repeat between transfers.
///
/// CFT wants a transfer ID "random enough not to collide" and the workspace deliberately
/// has no RNG dependency. `RandomState` is seeded from the OS per process, which is all a
/// transfer ID needs; mixing in the time and the file guards against two sends in one
/// process.
fn transfer_id(file: &[u8]) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write(file);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    h.write_u128(now.as_nanos());
    h.write_u32(std::process::id());
    h.finish().to_be_bytes().to_vec()
}

fn send(a: SendArgs) -> Result {
    let file = std::fs::read(&a.file).map_err(|e| format!("{}: {e}", a.file.display()))?;
    let name = a
        .name
        .clone()
        .or_else(|| a.file.file_name().map(|n| n.to_string_lossy().into_owned()));
    let mut opts = a.symbol.encode_options();
    // A transfer needs every symbol at one known size, so the version is fixed rather than
    // left to vary per chunk. 20 holds about a kilobyte at C1 and is readable at arm's length.
    let version = opts.version.unwrap_or(Version::new(20).map_err(err)?);
    opts.version = Some(version);
    let cap =
        lcr_encode::capacity_for_symbol(version, opts.profile, opts.ecc, opts.reference_border)
            .body_bytes;
    let id = transfer_id(&file);

    // CFT's chunk size is payload bytes, and each message adds its own CBOR framing — the
    // manifest more than a chunk. Start at the symbol's capacity and shrink by however much
    // the largest message overshot until every message encodes; that converges in a pass
    // or two, and it measures the real framing rather than assuming it.
    let mut chunk = cap as u32;
    let messages = loop {
        let t = lcr_cft::split(
            &file,
            name.clone(),
            id.clone(),
            lcr_cft::Compression::Deflate,
            chunk,
        )
        .map_err(err)?;
        let messages = t.messages();
        let largest = messages.iter().map(Vec::len).max().unwrap_or(0);
        if largest <= cap
            && let Some(m) = messages.iter().find(|m| m.len() == largest)
            && lcr_encode::encode_bytes(m, &opts).is_ok()
        {
            break messages;
        }
        let overshoot = (largest.saturating_sub(cap) as u32).max(8);
        chunk = chunk
            .checked_sub(overshoot)
            .filter(|c| *c >= 16)
            .ok_or_else(|| {
                format!(
                    "v{} {} is too small to carry a transfer; pass a larger --version",
                    version.get(),
                    profile_name(opts.profile)
                )
            })?;
    };

    std::fs::create_dir_all(&a.out_dir).map_err(|e| format!("{}: {e}", a.out_dir.display()))?;
    let stem = a
        .file
        .file_stem()
        .map_or("lcr".into(), |s| s.to_string_lossy().into_owned());
    let ropts = a.symbol.render_options();
    let width = messages.len().to_string().len().max(3);
    for (i, m) in messages.iter().enumerate() {
        let sym = lcr_encode::encode_bytes(m, &opts).map_err(|e| format!("message {i}: {e}"))?;
        let path = a.out_dir.join(format!("{stem}-{i:0width$}.png"));
        std::fs::write(&path, lcr_render::render_png(&sym, &ropts))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    eprintln!(
        "{} bytes → {} symbols (manifest + {} chunks), v{} {}, in {}",
        file.len(),
        messages.len(),
        messages.len() - 1,
        version.get(),
        profile_name(opts.profile),
        a.out_dir.display()
    );
    Ok(())
}

#[derive(Args)]
struct ReceiveArgs {
    /// Images of the symbols, in any order. Duplicates are fine.
    #[arg(required = true)]
    images: Vec<PathBuf>,
    /// Where to write the file. Default: the name the sender gave, in the current
    /// directory.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Overwrite the output if it exists.
    #[arg(long)]
    force: bool,
}

fn receive(a: ReceiveArgs) -> Result {
    let decoder = Decoder::new();
    let mut rx = lcr_cft::Reassembler::default();
    for p in &a.images {
        let accepted = load::load(p)
            .and_then(|f| decode_frame(&decoder, &f))
            .and_then(|o| payload(&o.packet).map(|(_, body)| body.to_vec()))
            .and_then(|body| rx.accept(&body).map_err(err));
        if let Err(e) = accepted {
            eprintln!("lcr: {}: skipped: {e}", p.display());
        }
    }
    match rx.status() {
        lcr_cft::Status::Complete => {}
        lcr_cft::Status::AwaitingManifest { held } => {
            return Err(format!(
                "the manifest (symbol 0) is missing; {held} chunk(s) are held until it arrives"
            ));
        }
        lcr_cft::Status::Receiving { have, total } => {
            // `lcr send` numbers files from the manifest at 0, so chunk i is file i + 1.
            let missing: Vec<String> = rx.missing().iter().map(|c| (c + 1).to_string()).collect();
            return Err(format!(
                "{have} of {total} chunks received; still need symbol(s) {}",
                missing.join(", ")
            ));
        }
    }
    let file = rx.finish().map_err(err)?;
    let out = match a.output {
        Some(p) => p,
        None => rx
            .manifest()
            .and_then(|m| m.safe_filename())
            .map(PathBuf::from)
            .ok_or("the sender gave no usable filename; pass --output")?,
    };
    if out.exists() && !a.force {
        return Err(format!(
            "{} exists; pass --force to overwrite",
            out.display()
        ));
    }
    std::fs::File::create(&out)
        .and_then(|mut f| f.write_all(&file))
        .map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!("{} bytes, hash verified → {}", file.len(), out.display());
    Ok(())
}
