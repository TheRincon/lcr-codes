//! QR generation across the component boundary, for `web/display.html`.
//!
//! A thin adapter over [`lcr_render::qr`], which owns the actual work — see that module for
//! why QR lives in `lcr-render` (§17.2 dual-render) and why generation is the `qrcodegen`
//! crate rather than code written here. Nothing is reimplemented at this layer, so the
//! page's comparison and a rendered dual are the same QR encoder verified by the same
//! round-trip tests.
//!
//! Behind the `compare` feature, which is off by default and in neither shipped artifact.

use crate::WasmError;
use lcr_render::qr;

pub use lcr_render::qr::{QrEcc, QrSymbol};

/// Encode bytes in QR byte mode.
///
/// Byte mode, unconditionally, because this is the *comparison* path: LCR Code carries
/// arbitrary bytes natively and §9.6 refuses a text-safe encoding layer precisely because
/// QR's alphanumeric restriction forces one on comparable systems. `lcr-render`'s
/// dual-render path uses mode-optimal text encoding instead, which is the right choice
/// there and the wrong one here.
pub fn qr_encode(data: &[u8], ecl: QrEcc) -> Result<QrSymbol, WasmError> {
    qr::qr_encode_bytes(data, ecl).map_err(|e| WasmError::QrTooLong(e.bytes as u32))
}
