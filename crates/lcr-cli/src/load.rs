//! Reading a capture from disk into RGB8 pixels.
//!
//! The format is decided by the file's leading bytes, never its extension: a phone that
//! saves a JPEG as `IMG_0001.heic`, or a screenshot tool that writes WebP under `.png`, is
//! common enough that trusting the name would be a bug report a week.
//!
//! Every decoder sits behind a cargo feature of the same name. Adding a format means one
//! arm in [`sniff`], one in [`load_bytes`] and one `decode_*` function — nothing else in
//! the CLI knows which formats exist.
//!
//! Colour images come out as packed RGB8 and grey ones as packed grey, because the LCR
//! decoder treats the two differently: a grey frame takes the luma-only path (§1.1 goal
//! 3), where the same pixels expanded to RGB would be read as a colour capture whose
//! chroma had collapsed. Alpha is composited over white, which is what the quiet zone of a
//! symbol saved with a transparent background is supposed to be.

use std::path::Path;

/// A decoded image, rows packed: `stride == width * format.channels()`.
pub struct Frame {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// `Rgb8` or `Gray8`.
    pub format: lcr_decode::image::PixelFormat,
}

/// The containers [`sniff`] recognises.
///
/// The first five are read. The rest are recognised only so the error can say what the
/// file is and print the command that converts it — a bare "not a recognised image" for a
/// photo straight off an iPhone would leave the user guessing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Jxl,
    Avif,
    /// The iPhone camera's default. There is no pure-Rust HEVC decoder; the C ones are
    /// LGPL, and HEVC itself sits in active patent pools — a poor fit for a project that
    /// licenses everything it can.
    Heic,
    /// HEIF whose brands do not name the codec.
    Heif,
    Gif,
    Bmp,
    /// TIFF, and the camera RAW formats built on it: DNG, CR2, NEF, ARW.
    Tiff,
    Jpeg2000,
    Psd,
    Pdf,
    /// Canon CR3, an ISO-BMFF RAW.
    Cr3,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Webp => "WebP",
            Format::Jxl => "JPEG XL",
            Format::Avif => "AVIF",
            Format::Heic => "HEIC",
            Format::Heif => "HEIF",
            Format::Gif => "GIF",
            Format::Bmp => "BMP",
            Format::Tiff => "TIFF (or a TIFF-based camera RAW: DNG, CR2, NEF, ARW)",
            Format::Jpeg2000 => "JPEG 2000",
            Format::Psd => "Photoshop (PSD)",
            Format::Pdf => "PDF",
            Format::Cr3 => "Canon RAW (CR3)",
        }
    }

    /// Advice beyond the conversion command, where there is any worth giving.
    fn tip(self) -> Option<&'static str> {
        match self {
            Format::Heic | Format::Heif => {
                Some("To avoid it on an iPhone, set Settings → Camera → Formats → Most Compatible.")
            }
            Format::Pdf => Some("Only the first page is converted."),
            _ => None,
        }
    }
}

/// Identify a container from its leading bytes.
pub fn sniff(data: &[u8]) -> Option<Format> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const JXL_CONTAINER: &[u8] = b"\0\0\0\x0cJXL \r\n\x87\n";
    const JP2_CONTAINER: &[u8] = b"\0\0\0\x0cjP  \r\n\x87\n";
    let format = if data.starts_with(PNG) {
        Format::Png
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Format::Jpeg
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Format::Webp
    } else if data.starts_with(&[0xff, 0x0a]) || data.starts_with(JXL_CONTAINER) {
        Format::Jxl
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Format::Gif
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        Format::Tiff
    } else if data.starts_with(JP2_CONTAINER) || data.starts_with(&[0xff, 0x4f, 0xff, 0x51]) {
        Format::Jpeg2000
    } else if data.starts_with(b"8BPS") {
        Format::Psd
    } else if data.starts_with(b"%PDF-") {
        Format::Pdf
    } else if is_bmp(data) {
        Format::Bmp
    } else {
        return iso_bmff_brand(data);
    };
    Some(format)
}

/// `BM` alone is two bytes any file could start with, so the DIB header's size field must
/// also be one of the sizes the format defines.
fn is_bmp(data: &[u8]) -> bool {
    data.len() >= 18
        && data.starts_with(b"BM")
        && matches!(
            u32::from_le_bytes([data[14], data[15], data[16], data[17]]),
            12 | 40 | 52 | 56 | 64 | 108 | 124
        )
}

/// AVIF, HEIC and CR3 are all ISO-BMFF: an `ftyp` box whose major or compatible brands say
/// which. Brands are checked in order, so a file that lists `avif` before `mif1` is AVIF;
/// the generic HEIF brands only decide when nothing more specific appears.
fn iso_bmff_brand(data: &[u8]) -> Option<Format> {
    if data.len() < 16 || &data[4..8] != b"ftyp" {
        return None;
    }
    let size = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
    let end = size.clamp(16, data.len());
    // Major brand at 8..12, minor version at 12..16, compatible brands from 16.
    let major: &[u8; 4] = data[8..12].try_into().ok()?;
    let brands = std::iter::once(major).chain(data[16..end].as_chunks::<4>().0);
    let mut generic = None;
    for brand in brands {
        match brand {
            b"avif" | b"avis" => return Some(Format::Avif),
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" => return Some(Format::Heic),
            b"crx " => return Some(Format::Cr3),
            b"mif1" | b"msf1" => generic = Some(Format::Heif),
            _ => {}
        }
    }
    generic
}

/// Read and decode an image file. Errors do not name the path; the caller does.
///
/// A format `lcr` recognises but cannot read gets a conversion command for this file.
pub fn load(path: &Path) -> Result<Frame, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    if let Some(f) = sniff(&data)
        && !f.readable()
    {
        let mut msg = format!(
            "{} is not supported. Convert it to PNG first:\n    {}",
            f.name(),
            convert_command(path)
        );
        if let Some(tip) = f.tip() {
            msg.push('\n');
            msg.push_str(tip);
        }
        return Err(msg);
    }
    load_bytes(&data)
}

impl Format {
    /// Whether `lcr` reads this format at all, compiled in or not.
    fn readable(self) -> bool {
        matches!(
            self,
            Format::Png | Format::Jpeg | Format::Webp | Format::Jxl | Format::Avif
        )
    }
}

/// The command that turns `path` into a PNG beside it: `sips` on macOS, where it ships
/// with the system and reads every format above, and ImageMagick elsewhere.
fn convert_command(path: &Path) -> String {
    let mut out = path.with_extension("png");
    // A file named `.png` that is really something else would be converted onto itself.
    if out == path {
        let stem = path
            .file_stem()
            .map_or("image".into(), |s| s.to_string_lossy());
        out = path.with_file_name(format!("{stem}-converted.png"));
    }
    if cfg!(target_os = "macos") {
        format!("sips -s format png {} --out {}", quote(path), quote(&out))
    } else {
        // `[0]`: the first frame of a GIF, page of a PDF or TIFF, or a PSD's flattened
        // composite, where ImageMagick would otherwise write one PNG per frame or layer.
        let first = format!("{}[0]", path.display());
        format!(
            "magick {} {}    # ImageMagick",
            quote(Path::new(&first)),
            quote(&out)
        )
    }
}

/// Quote a path for a POSIX shell, only when it needs it.
fn quote(path: &Path) -> String {
    let s = path.to_string_lossy();
    let plain = |c: char| c.is_ascii_alphanumeric() || "._-/+,:@%".contains(c);
    if !s.is_empty() && s.chars().all(plain) {
        s.into_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Decode an image already in memory.
pub fn load_bytes(data: &[u8]) -> Result<Frame, String> {
    let format = sniff(data).ok_or("not a recognised image (PNG, JPEG, WebP, JPEG XL or AVIF)")?;
    let missing = |feature: &str| {
        Err(format!(
            "{} support is not compiled into this build (cargo feature `{feature}`)",
            format.name()
        ))
    };
    match format {
        #[cfg(feature = "png")]
        Format::Png => decode_png(data),
        #[cfg(feature = "jpeg")]
        Format::Jpeg => decode_jpeg(data),
        #[cfg(feature = "webp")]
        Format::Webp => decode_webp(data),
        #[cfg(feature = "jxl")]
        Format::Jxl => decode_jxl(data),
        #[cfg(feature = "avif")]
        Format::Avif => decode_avif(data),
        // `load` replaces this with the conversion command for the file.
        f @ (Format::Heic
        | Format::Heif
        | Format::Gif
        | Format::Bmp
        | Format::Tiff
        | Format::Jpeg2000
        | Format::Psd
        | Format::Pdf
        | Format::Cr3) => Err(format!(
            "{} is not supported; convert it to PNG first",
            f.name()
        )),
        #[allow(unreachable_patterns)]
        Format::Png => missing("png"),
        #[allow(unreachable_patterns)]
        Format::Jpeg => missing("jpeg"),
        #[allow(unreachable_patterns)]
        Format::Webp => missing("webp"),
        #[allow(unreachable_patterns)]
        Format::Jxl => missing("jxl"),
        #[allow(unreachable_patterns)]
        Format::Avif => missing("avif"),
    }
}

// ---------------------------------------------------------------- pixel shapes

/// Pack interleaved 8-bit samples with `channels` per pixel: 1 grey, 2 grey + alpha,
/// 3 RGB, 4 RGBA. Grey stays grey and colour becomes RGB8; alpha is composited over white.
fn to_rgb(samples: &[u8], channels: usize, width: u32, height: u32) -> Result<Frame, String> {
    let n = width as usize * height as usize;
    if samples.len() < n * channels {
        return Err(format!(
            "decoder returned {} bytes for a {width}×{height}×{channels} image",
            samples.len()
        ));
    }
    use lcr_decode::image::PixelFormat;
    let over_white =
        |c: u8, a: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32) + 127) / 255) as u8;
    let samples = &samples[..n * channels];
    let (pixels, format) = match channels {
        1 => (samples.to_vec(), PixelFormat::Gray8),
        2 => (
            samples
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| over_white(p[0], p[1]))
                .collect(),
            PixelFormat::Gray8,
        ),
        3 => (samples.to_vec(), PixelFormat::Rgb8),
        4 => (
            samples
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    [
                        over_white(p[0], p[3]),
                        over_white(p[1], p[3]),
                        over_white(p[2], p[3]),
                    ]
                })
                .collect(),
            PixelFormat::Rgb8,
        ),
        _ => return Err(format!("unsupported channel count {channels}")),
    };
    Ok(Frame {
        pixels,
        width,
        height,
        format,
    })
}

// ---------------------------------------------------------------- decoders

#[cfg(feature = "png")]
fn decode_png(data: &[u8]) -> Result<Frame, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    // Palette and low-bit-depth images expanded, 16-bit stripped: always 8-bit samples.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let size = reader
        .output_buffer_size()
        .ok_or("PNG too large to decode")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let channels = info.color_type.samples();
    to_rgb(&buf, channels, info.width, info.height)
}

#[cfg(feature = "jpeg")]
fn decode_jpeg(data: &[u8]) -> Result<Frame, String> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(data), options);
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("JPEG has no frame header")?;
    let channels = decoder
        .output_colorspace()
        .map_or(3, |c| c.num_components());
    to_rgb(&pixels, channels, info.width.into(), info.height.into())
}

#[cfg(feature = "webp")]
fn decode_webp(data: &[u8]) -> Result<Frame, String> {
    let mut decoder =
        image_webp::WebPDecoder::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let (width, height) = decoder.dimensions();
    let channels = if decoder.has_alpha() { 4 } else { 3 };
    let size = decoder
        .output_buffer_size()
        .ok_or("WebP too large to decode")?;
    let mut buf = vec![0; size];
    // An animated WebP yields its first frame, which is the right answer for a still capture.
    decoder.read_image(&mut buf).map_err(|e| e.to_string())?;
    to_rgb(&buf, channels, width, height)
}

#[cfg(feature = "jxl")]
fn decode_jxl(data: &[u8]) -> Result<Frame, String> {
    use jxl_oxide::{EnumColourEncoding, JxlImage, RenderingIntent};
    let mut image = JxlImage::builder()
        .read(std::io::Cursor::new(data))
        .map_err(|e| e.to_string())?;
    // XYB-encoded images have no "natural" output space; ask for sRGB, which is what the
    // LCR decoder's colour model assumes of every capture.
    image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    let render = image.render_frame(0).map_err(|e| e.to_string())?;
    let mut stream = render.stream();
    let (width, height, channels) = (stream.width(), stream.height(), stream.channels() as usize);
    let mut buf = vec![0u8; width as usize * height as usize * channels];
    stream.write_to_buffer(&mut buf);
    to_rgb(&buf, channels, width, height)
}

#[cfg(feature = "avif")]
fn decode_avif(data: &[u8]) -> Result<Frame, String> {
    let image = avif_decode::Decoder::from_avif(data)
        .and_then(|d| d.to_image())
        .map_err(|e| e.to_string())?;
    // 16-bit samples keep their top byte: an 8-bit capture is all the decoder uses.
    let hi = |v: u16| (v >> 8) as u8;
    let (samples, channels, width, height): (Vec<u8>, usize, usize, usize) = match image {
        avif_decode::Image::Rgb8(i) => (
            i.pixels().flat_map(|p| [p.r, p.g, p.b]).collect(),
            3,
            i.width(),
            i.height(),
        ),
        avif_decode::Image::Rgba8(i) => (
            i.pixels().flat_map(|p| [p.r, p.g, p.b, p.a]).collect(),
            4,
            i.width(),
            i.height(),
        ),
        avif_decode::Image::Gray8(i) => (
            i.pixels().map(|p| p.value()).collect(),
            1,
            i.width(),
            i.height(),
        ),
        avif_decode::Image::Rgb16(i) => (
            i.pixels()
                .flat_map(|p| [hi(p.r), hi(p.g), hi(p.b)])
                .collect(),
            3,
            i.width(),
            i.height(),
        ),
        avif_decode::Image::Rgba16(i) => (
            i.pixels()
                .flat_map(|p| [hi(p.r), hi(p.g), hi(p.b), hi(p.a)])
                .collect(),
            4,
            i.width(),
            i.height(),
        ),
        avif_decode::Image::Gray16(i) => (
            i.pixels().map(|p| hi(p.value())).collect(),
            1,
            i.width(),
            i.height(),
        ),
    };
    to_rgb(&samples, channels, width as u32, height as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_by_content_not_name() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some(Format::Png));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some(Format::Jpeg));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::Webp));
        assert_eq!(sniff(&[0xff, 0x0a, 0x00]), Some(Format::Jxl));
        assert_eq!(sniff(b"\0\0\0\x0cJXL \r\n\x87\n\0\0"), Some(Format::Jxl));
        assert_eq!(
            sniff(b"\0\0\0\x1cftypavif\0\0\0\0mif1miaf"),
            Some(Format::Avif)
        );
        // HEIC whose major brand is the generic `mif1`: found among the compatible brands.
        assert_eq!(
            sniff(b"\0\0\0\x18ftypmif1\0\0\0\0heicmif1"),
            Some(Format::Heic)
        );
        assert_eq!(sniff(b"GIF89a"), Some(Format::Gif));
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"hello, world"), None);
    }

    #[test]
    fn recognises_what_it_cannot_read() {
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        assert_eq!(sniff(&bmp), Some(Format::Bmp));
        assert_eq!(
            sniff(b"BMW is not a bitmap at all"),
            None,
            "`BM` alone is not enough"
        );
        assert_eq!(sniff(b"II*\0\x08\0\0\0"), Some(Format::Tiff));
        assert_eq!(sniff(b"MM\0*\0\0\0\x08"), Some(Format::Tiff));
        assert_eq!(sniff(b"\0\0\0\x0cjP  \r\n\x87\n\0"), Some(Format::Jpeg2000));
        assert_eq!(sniff(b"8BPS\0\x01"), Some(Format::Psd));
        assert_eq!(sniff(b"%PDF-1.7\n"), Some(Format::Pdf));
        assert_eq!(
            sniff(b"\0\0\0\x18ftypcrx \0\0\0\x01crx isom"),
            Some(Format::Cr3)
        );
        // Only generic HEIF brands: still HEIF, not unrecognised.
        assert_eq!(
            sniff(b"\0\0\0\x18ftypmif1\0\0\0\0mif1miaf"),
            Some(Format::Heif)
        );
    }

    /// A file of `bytes` under the system temp dir, removed on drop.
    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new(name: &str, bytes: &[u8]) -> TempFile {
            let dir = std::env::temp_dir().join(format!("lcr-load-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            TempFile(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn unreadable_formats_get_a_conversion_command() {
        let f = TempFile::new("IMG 0001.heic", b"\0\0\0\x18ftypheic\0\0\0\0mif1heic");
        let e = load(&f.0).err().unwrap();
        assert!(e.starts_with("HEIC is not supported"), "{e}");
        assert!(e.contains("Most Compatible"), "{e}");
        // The path has a space, so it is quoted, and the output sits beside it as PNG.
        let out = f.0.with_extension("png");
        if cfg!(target_os = "macos") {
            assert!(
                e.contains(&format!(
                    "sips -s format png '{}' --out '{}'",
                    f.0.display(),
                    out.display()
                )),
                "{e}"
            );
        } else {
            assert!(
                e.contains(&format!(
                    "magick '{}[0]' '{}'",
                    f.0.display(),
                    out.display()
                )),
                "{e}"
            );
        }

        // A GIF misnamed `.png` must not be converted onto itself.
        let g = TempFile::new("disguised.png", b"GIF89a\x01\0\x01\0");
        let e = load(&g.0).err().unwrap();
        assert!(e.contains("disguised-converted.png"), "{e}");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote(Path::new("a/b-c_1.png")), "a/b-c_1.png");
        assert_eq!(quote(Path::new("my photo.heic")), "'my photo.heic'");
        assert_eq!(quote(Path::new("it's.gif")), r"'it'\''s.gif'");
    }

    #[test]
    fn alpha_composites_over_white() {
        let f = to_rgb(&[0, 0, 0, 0, 10, 20, 30, 255], 4, 2, 1).unwrap();
        assert_eq!(f.pixels, [255, 255, 255, 10, 20, 30]);
        assert_eq!(f.format, lcr_decode::image::PixelFormat::Rgb8);
        let f = to_rgb(&[7, 128], 2, 1, 1).unwrap();
        assert_eq!(f.pixels, [131], "grey with alpha stays grey");
        assert_eq!(f.format, lcr_decode::image::PixelFormat::Gray8);
    }

    #[cfg(feature = "png")]
    #[test]
    fn reads_a_png_the_renderer_wrote() {
        let raster = lcr_render::Raster {
            width: 2,
            height: 1,
            data: vec![255, 0, 0, 0, 0, 255],
        };
        let f = load_bytes(&lcr_render::png_encode(&raster)).unwrap();
        assert_eq!((f.width, f.height), (2, 1));
        assert_eq!(f.pixels, raster.data);
    }
}
