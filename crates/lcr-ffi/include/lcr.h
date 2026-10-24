/*
 * LCR Code — C API.
 *
 * Generated from crates/lcr-ffi/src/lib.rs by `just ffi-header`. Do not edit; edit the
 * Rust source and regenerate. Conventions (status codes, ownership, threads) are in the
 * crate documentation at the top of that file.
 *
 * SPDX-License-Identifier: MIT OR Apache-2.0 OR Unlicense
 */

#ifndef LCR_H
#define LCR_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Multi-frame LLR combining (§12.4). Opaque; create with `lcr_accumulator_new`.
//
// The spec calls this "the single best use of a live camera preview": banding and glare
// move between frames, so accumulating soft information across them is far cheaper than
// tightening the code rate.
typedef struct LcrAccumulator LcrAccumulator;

#if defined(LCR_FOUNTAIN)
// A fountain transfer (CFT.md §21): the source chunks once, then repair symbols without
// end, any slightly-more-than-`source_count` of which rebuild the file. Opaque; create
// with `lcr_cft_fountain_new`, release with `lcr_cft_fountain_free`.
typedef struct LcrCftFountain LcrCftFountain;
#endif

// Collects CFT messages in any order and reconstructs the file. Opaque; create with
// `lcr_cft_receiver_new`, release with `lcr_cft_receiver_free`.
typedef struct LcrCftReceiver LcrCftReceiver;

// A file, compressed and split into CFT messages (docs/CFT.md). Opaque; create with
// `lcr_cft_split`, release with `lcr_cft_transfer_free`.
typedef struct LcrCftTransfer LcrCftTransfer;

// A byte buffer owned by the caller once returned. Release with `lcr_bytes_free`.
//
// An empty buffer has `data == NULL` and `len == 0`.
typedef struct {
  uint8_t *data;
  size_t len;
} LcrBytes;

// What a decode produced. One of the `LCR_PACKET_*` values.
typedef uint32_t LcrPacketKind;

// Wire content type (§9.2). One of the `LCR_CONTENT_*` values.
typedef uint32_t LcrContentType;

// Colour configuration (§5). One of the `LCR_PROFILE_*` values.
typedef uint32_t LcrProfile;

// Why a decode went the way it did (§14.3).
typedef struct {
  uint16_t luma_iterations;
  uint16_t chroma_iterations;
  // False means the chroma layer was unrecoverable.
  bool chroma_recovered;
  // RMS residual of the §12.3 illumination fit, in linear light.
  float illum_residual_rms;
  // References discarded by §12.3's robust refit. High means glare.
  uint8_t references_rejected;
  // Per-module luma SNR. The most useful single number for user guidance.
  float snr_db;
  // Frames combined before decoding (§12.4).
  uint8_t frames_combined;
  // Symbol version, 1..=64.
  uint8_t version;
  // One of the `LCR_PROFILE_*` values.
  LcrProfile profile;
  // Confidence margin of the winning format-region copy.
  float format_margin;
  // Whether a full homography was fitted rather than an affine fallback.
  bool alignment_found;
  // Module pitch as captured, in sensor pixels, at the symbol's finest corner.
  float capture_pitch_min;
  // Module pitch as captured at the coarsest corner. The ratio against
  // `capture_pitch_min` measures viewing obliquity.
  float capture_pitch_max;
} LcrDiagnostics;

// The result of a decode (§14.3). Release with `lcr_decode_result_free`.
typedef struct {
  // One of the `LCR_PACKET_*` values. Read it before reading anything else.
  LcrPacketKind kind;
  // One of the `LCR_CONTENT_*` values. Meaningful for `UNSIGNED` and `VERIFIED` only.
  LcrContentType ctype;
  // The payload, for `UNSIGNED` and `VERIFIED`. Empty otherwise.
  LcrBytes body;
  // The luma-only prefix, for `PARTIAL`. Empty otherwise. Never content (§8.1).
  LcrBytes prefix;
  LcrDiagnostics diagnostics;
} LcrDecodeResult;

// Result of a fallible call. `LCR_OK` is zero; see `lcr_last_error()` otherwise.
typedef uint32_t LcrStatus;

// Pixel layout of a supplied frame. One of the `LCR_PIXEL_*` values.
typedef uint32_t LcrPixelFormat;

// A captured frame, borrowed for the duration of a call.
typedef struct {
  // Pixel data, at least `(height - 1) * stride + width * channels` bytes.
  const uint8_t *pixels;
  // Length of `pixels` in bytes.
  size_t len;
  uint32_t width;
  uint32_t height;
  // Bytes per row. May exceed `width * channels` for a padded camera plane.
  uint32_t stride;
  // One of the `LCR_PIXEL_*` values.
  LcrPixelFormat format;
} LcrImage;

// Where a symbol is and how it was captured, established without decoding it.
//
// Every field also appears in `LcrDiagnostics`. These are the ones that survive a decode
// failure: they come from §12.1–§12.3, which run before any payload is touched.
typedef struct {
  uint8_t version;
  // One of the `LCR_PROFILE_*` values.
  LcrProfile profile;
  float format_margin;
  bool alignment_found;
  float illum_residual_rms;
  uint8_t references_rejected;
  float capture_pitch_min;
  float capture_pitch_max;
} LcrProbeResult;

// Compression applied to the framed body (§9.3). One of the `LCR_COMPRESSION_*` values.
typedef uint32_t LcrCompression;

// Encoding knobs. Start from `lcr_encode_options_default()` rather than zeroing the
// struct: zero is a valid value for every field, but it means monochrome, ECC level 0, no
// border and the fixed mask 0 — a legal symbol, and not the one anyone wants by accident.
typedef struct {
  // One of the `LCR_PROFILE_*` values.
  LcrProfile profile;
  // Symbol version 1..=64, or 0 for the smallest that fits (§6.1). Versions above 32 are
  // ring-only (D21): §7's classic format word has five bits for the version.
  uint8_t version;
  // Luma error-correction level.
  uint8_t ecc_luma;
  // Chroma error-correction level.
  uint8_t ecc_chroma;
  // One of the `LCR_COMPRESSION_*` values.
  LcrCompression compression;
  // Emit the §6.4 screen reference border. Costs no payload capacity — it lives outside
  // the symbol grid — and materially improves decoding under glare.
  bool reference_border;
  // Data mask, or -1 for the §8.5 penalty search.
  int16_t mask;
} LcrEncodeOptions;

// A rendered symbol. Release with `lcr_rendered_free`.
typedef struct {
  // Row-major RGB triples, `width * height * 3` bytes.
  LcrBytes pixels;
  uint32_t width;
  uint32_t height;
  // Symbol version actually used.
  uint8_t version;
  // Grid side in modules, excluding the quiet zone.
  uint32_t side;
} LcrRendered;

// Where a receive has got to. One of the `LCR_CFT_*` values.
typedef uint32_t LcrCftState;

// How much of a transfer has arrived.
typedef struct {
  // One of the `LCR_CFT_*` values.
  LcrCftState state;
  // Chunks held before the manifest arrived. `AWAITING_MANIFEST` only.
  uint32_t held;
  // Chunks arrived. `RECEIVING` only.
  uint32_t have;
  // Chunks in the transfer. `RECEIVING` only.
  uint32_t total;
} LcrCftStatus;

// Success.
#define LCR_OK 0

// A required pointer was null, text was not UTF-8, or an enum or option was out of range.
// The call was rejected before doing any work.
#define LCR_ERR_ARGUMENT 1

// The operation itself failed: no symbol in the frame, a payload too large for any
// version, a malformed or incomplete transfer.
#define LCR_ERR_FAILED 2

// Monochrome fallback, 1.00 bits/module.
#define LCR_PROFILE_MONO 0

// Screen default: 4 colours, co-sited chroma.
#define LCR_PROFILE_C1 1

// Print default: 4 colours, 2×2 chroma cells.
#define LCR_PROFILE_C2 2

// 8 colours, co-sited chroma. Optional, and the least robust.
#define LCR_PROFILE_C3 3

// Opaque bytes, no interpretation.
#define LCR_CONTENT_OPAQUE 0

// UTF-8 text.
#define LCR_CONTENT_TEXT 1

// JSON. Discouraged on the wire (§9.4); `lcr_encode_json` sends CBOR instead.
#define LCR_CONTENT_JSON 2

// CBOR — what `lcr_encode_json` produces. `lcr_cbor_to_json` renders it.
#define LCR_CONTENT_CBOR 3

// COSE_Sign1, the optional signed packet (§10).
#define LCR_CONTENT_COSE_SIGN1 4

// Multipart (§9.5).
#define LCR_CONTENT_MULTIPART 5

// One byte per pixel. Chroma is unavailable, so only the luma layer decodes.
#define LCR_PIXEL_GRAY8 0

// Three bytes per pixel.
#define LCR_PIXEL_RGB8 1

// Four bytes per pixel; the fourth is ignored.
#define LCR_PIXEL_RGBA8 2

// Try each scheme and keep the smallest — frequently none at these sizes. Zero, so a
// zero-initialised `LcrEncodeOptions` asks for it.
#define LCR_COMPRESSION_AUTO 0

// No compression.
#define LCR_COMPRESSION_NONE 1

// DEFLATE (RFC 1951).
#define LCR_COMPRESSION_DEFLATE 2

// Brotli. Not compiled into this build, so encoding with it fails; listed so the values
// match the format rather than this build.
#define LCR_COMPRESSION_BROTLI 3

// Decoded and carries no signature — the §1.2 common case. `body` holds the payload.
#define LCR_PACKET_UNSIGNED 0

// Signature verified. `body` holds the payload.
#define LCR_PACKET_VERIFIED 1

// Well-formed COSE, not verified. `body` is empty by construction (§10.8): there is no
// unverified content to read by forgetting to check this field.
#define LCR_PACKET_UNVERIFIED 2

// A luma-only prefix (§12.5), in `prefix`. A **diagnostic**, not content: it has no CRC
// to check until the whole frame arrives, and §8.1 forbids acting on it.
#define LCR_PACKET_PARTIAL 3

// No manifest yet, so chunks cannot be counted. `held` says how many are waiting.
#define LCR_CFT_AWAITING_MANIFEST 0

// Manifest in hand; `have` of `total` chunks arrived.
#define LCR_CFT_RECEIVING 1

// Every chunk has arrived; call `lcr_cft_receiver_finish`.
#define LCR_CFT_COMPLETE 2

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

// Describe the most recent failure on this thread, or null if there has been none.
//
// The string is owned by the library and stays valid until the next failing call on
// the same thread. It is only meaningful straight after a call returned something other
// than `LCR_OK`: successful calls do not clear it.
const char *lcr_last_error(void);

// The library version, e.g. `"0.4.0"`. Static; never free it.
const char *lcr_version(void);

// Release a buffer the library returned and reset it to empty. Null is ignored.
//
// # Safety
//
// `bytes` must be null or point to an `LcrBytes` the library filled in and that has not
// been modified since.
void lcr_bytes_free(LcrBytes *bytes);

// Release a string the library returned. Null is ignored.
//
// # Safety
//
// `s` must be null or a string returned by this library and not yet freed. Never pass
// the result of `lcr_last_error()` or `lcr_version()`, which the library owns.
void lcr_string_free(char *s);

// Release the buffers inside a decode result and reset them to empty. Null is ignored.
//
// # Safety
//
// `result` must be null or point to a result the library filled in.
void lcr_decode_result_free(LcrDecodeResult *result);

// Decode a single frame.
//
// # Safety
//
// `img` must point to a valid `LcrImage` whose `pixels` are readable for `len` bytes.
// `out` must be valid for writes.
LcrStatus lcr_decode(const LcrImage *img, LcrDecodeResult *out);

// Locate a symbol and measure the capture geometry, without decoding it.
//
// §12.1–§12.3 only. Succeeding means a symbol was found and its format region read, and
// says nothing about the payload — which is what separates "nothing in frame" from
// "found it, move closer" in a scanning UI.
//
// # Safety
//
// As `lcr_decode`.
LcrStatus lcr_probe(const LcrImage *img, LcrProbeResult *out);

// Create an empty accumulator. Release with `lcr_accumulator_free`.
LcrAccumulator *lcr_accumulator_new(void);

// Release an accumulator. Null is ignored.
//
// # Safety
//
// `acc` must be null or a live handle from `lcr_accumulator_new`.
void lcr_accumulator_free(LcrAccumulator *acc);

// Add a frame.
//
// # Safety
//
// `acc` must be a live handle; `img` as for `lcr_decode`.
LcrStatus lcr_accumulator_push(LcrAccumulator *acc, const LcrImage *img);

// How many frames have been combined. Zero for a null handle.
//
// # Safety
//
// `acc` must be null or a live handle.
uint8_t lcr_accumulator_frames(const LcrAccumulator *acc);

// Attempt a decode from everything pushed so far.
//
// # Safety
//
// `acc` must be a live handle and `out` valid for writes.
LcrStatus lcr_accumulator_try_decode(const LcrAccumulator *acc, LcrDecodeResult *out);

// Discard accumulated frames. Null is ignored.
//
// # Safety
//
// `acc` must be null or a live handle.
void lcr_accumulator_reset(LcrAccumulator *acc);

// The defaults the WebAssembly component uses: C1, automatic version, ECC level 1 on both
// layers, automatic compression, reference border on, automatic mask.
LcrEncodeOptions lcr_encode_options_default(void);

// Release the pixels inside a rendered symbol. Null is ignored.
//
// # Safety
//
// `r` must be null or point to a symbol the library rendered.
void lcr_rendered_free(LcrRendered *r);

// Encode JSON and render it as RGB pixels. The JSON is converted to CBOR first — §9.4's
// "author in JSON, transmit CBOR" — so it decodes as `LCR_CONTENT_CBOR`.
//
// `json` need not be NUL-terminated. `opts` may be null for the defaults. `module_px` is
// the pitch in pixels per module, at least 8.
//
// # Safety
//
// `json` must be readable for `len` bytes, `opts` null or valid, `out` valid for writes.
LcrStatus lcr_encode_json(const char *json,
                          size_t len,
                          const LcrEncodeOptions *opts,
                          uint32_t module_px,
                          LcrRendered *out);

// Encode UTF-8 text and render it as RGB pixels.
//
// # Safety
//
// As `lcr_encode_json`.
LcrStatus lcr_encode_text(const char *text,
                          size_t len,
                          const LcrEncodeOptions *opts,
                          uint32_t module_px,
                          LcrRendered *out);

// Encode opaque bytes and render them as RGB pixels.
//
// # Safety
//
// `data` must be readable for `len` bytes; otherwise as `lcr_encode_json`.
LcrStatus lcr_encode_bytes(const uint8_t *data,
                           size_t len,
                           const LcrEncodeOptions *opts,
                           uint32_t module_px,
                           LcrRendered *out);

// Encode JSON and render it as an SVG document. Release `*out_svg` with
// `lcr_string_free`.
//
// # Safety
//
// As `lcr_encode_json`, with `out_svg` valid for writes.
LcrStatus lcr_encode_json_svg(const char *json,
                              size_t len,
                              const LcrEncodeOptions *opts,
                              uint32_t module_px,
                              char **out_svg);

// Largest framed body a symbol with these settings holds, in bytes.
//
// For sizing CFT chunks from the carrier: the alternative — encode, catch the failure,
// try a smaller chunk — turns a lookup into a search.
//
// # Safety
//
// `out` must be valid for writes.
LcrStatus lcr_capacity(LcrProfile profile,
                       uint8_t version,
                       uint8_t ecc_luma,
                       uint8_t ecc_chroma,
                       bool reference_border,
                       uint32_t *out);

// Render a CBOR body as JSON text — the read side of §9.4. Release `*out_json` with
// `lcr_string_free`.
//
// # Safety
//
// `cbor` must be readable for `len` bytes and `out_json` valid for writes.
LcrStatus lcr_cbor_to_json(const uint8_t *cbor, size_t len, char **out_json);

// The exact bytes `lcr_encode_json` puts on the wire — §9.4's write side. Release
// `out` with `lcr_bytes_free`.
//
// # Safety
//
// `json` must be readable for `len` bytes and `out` valid for writes.
LcrStatus lcr_json_to_cbor(const char *json, size_t len, LcrBytes *out);

// Compress `file` and split it into chunks of at most `chunk_size` payload bytes, one
// symbol's worth each. Message 0 is the manifest.
//
// `filename` is a NUL-terminated UTF-8 string, or null for none. `transfer_id`
// identifies the transfer and should be random enough not to collide with another;
// this library has no RNG of its own. Size `chunk_size` with `lcr_capacity`.
//
// # Safety
//
// `file` and `transfer_id` must be readable for their lengths, `filename` null or a valid
// C string, and `out` valid for writes.
LcrStatus lcr_cft_split(const uint8_t *file,
                        size_t file_len,
                        const char *filename,
                        const uint8_t *transfer_id,
                        size_t transfer_id_len,
                        uint32_t chunk_size,
                        LcrCftTransfer **out);

#if defined(LCR_FOUNTAIN)
// Compress and split `file` as `lcr_cft_split` does, and wrap the chunks as a fountain.
// Arguments are as for `lcr_cft_split`.
//
// # Safety
//
// As for `lcr_cft_split`.
LcrStatus lcr_cft_fountain_new(const uint8_t *file,
                               size_t file_len,
                               const char *filename,
                               const uint8_t *transfer_id,
                               size_t transfer_id_len,
                               uint32_t chunk_size,
                               LcrCftFountain **out);
#endif

#if defined(LCR_FOUNTAIN)
// Release a fountain. Null is ignored.
//
// # Safety
//
// `f` must be null or a live handle from `lcr_cft_fountain_new`.
void lcr_cft_fountain_free(LcrCftFountain *f);
#endif

#if defined(LCR_FOUNTAIN)
// The message for frame `n` of the carousel, counting from 0: the manifest every eighth
// frame, otherwise the source chunks once and then repair symbols. Encode each with
// `lcr_encode_bytes` and show them in order. Free `out` with `lcr_bytes_free`.
//
// # Safety
//
// `f` must be a live handle and `out` valid for writes.
LcrStatus lcr_cft_fountain_frame(const LcrCftFountain *f, uint64_t n, LcrBytes *out);
#endif

#if defined(LCR_FOUNTAIN)
// Source chunk count, `K`. Zero for null.
//
// # Safety
//
// `f` must be null or a live handle.
uint32_t lcr_cft_fountain_source_count(const LcrCftFountain *f);
#endif

#if defined(LCR_FOUNTAIN)
// The file's size before compression. Zero for null.
//
// # Safety
//
// `f` must be null or a live handle.
uint64_t lcr_cft_fountain_original_size(const LcrCftFountain *f);
#endif

#if defined(LCR_FOUNTAIN)
// The compressed stream's size. Zero for null.
//
// # Safety
//
// `f` must be null or a live handle.
uint64_t lcr_cft_fountain_compressed_size(const LcrCftFountain *f);
#endif

// Release a transfer. Null is ignored.
//
// # Safety
//
// `t` must be null or a live handle from `lcr_cft_split`.
void lcr_cft_transfer_free(LcrCftTransfer *t);

// How many messages the transfer takes, manifest included. Zero for null.
//
// # Safety
//
// `t` must be null or a live handle.
size_t lcr_cft_transfer_message_count(const LcrCftTransfer *t);

// Borrow message `index`. The bytes stay owned by the transfer and valid until it is
// freed; encode each with `lcr_encode_bytes` to get one symbol per message.
//
// # Safety
//
// `t` must be a live handle and `data` and `len` valid for writes.
LcrStatus lcr_cft_transfer_message(const LcrCftTransfer *t,
                                   size_t index,
                                   const uint8_t **data,
                                   size_t *len);

// Chunks after the manifest. 1 when the whole file fits in the manifest itself. Zero for
// null.
//
// # Safety
//
// `t` must be null or a live handle.
uint32_t lcr_cft_transfer_total_chunks(const LcrCftTransfer *t);

// The file's size before compression. Zero for null.
//
// # Safety
//
// `t` must be null or a live handle.
uint64_t lcr_cft_transfer_original_size(const LcrCftTransfer *t);

// The compressed stream's size. Zero for null.
//
// # Safety
//
// `t` must be null or a live handle.
uint64_t lcr_cft_transfer_compressed_size(const LcrCftTransfer *t);

// Create an empty receiver, with the default size limits (64 MiB).
LcrCftReceiver *lcr_cft_receiver_new(void);

// Release a receiver. Null is ignored.
//
// # Safety
//
// `r` must be null or a live handle from `lcr_cft_receiver_new`.
void lcr_cft_receiver_free(LcrCftReceiver *r);

// Feed one decoded CFT message — manifest, chunk, or with the `fountain` feature a repair
// symbol — in any order.
// `out` may be null; otherwise it receives the status after this message.
//
// # Safety
//
// `r` must be a live handle, `bytes` readable for `len` bytes, `out` null or valid for
// writes.
LcrStatus lcr_cft_receiver_accept(LcrCftReceiver *r,
                                  const uint8_t *bytes,
                                  size_t len,
                                  LcrCftStatus *out);

// The receiver's current status. A null handle reads as awaiting a manifest.
//
// # Safety
//
// `r` must be null or a live handle.
LcrCftStatus lcr_cft_receiver_status(const LcrCftReceiver *r);

#if defined(LCR_FOUNTAIN)
// Fountain repair symbols held that have not yet resolved to a chunk. Zero for null.
//
// # Safety
//
// `r` must be null or a live handle.
uint32_t lcr_cft_receiver_pending(const LcrCftReceiver *r);
#endif

// Chunk indices still outstanding (CFT.md §10) — what a receiver shows so the sender
// knows which symbols to display again.
//
// Writes up to `cap` indices to `out` and returns how many are missing in total, so a
// caller can size a buffer with a first call passing `cap = 0`. Zero for a null handle,
// and zero before the manifest arrives, since until then nothing can be counted.
//
// # Safety
//
// `r` must be null or a live handle; `out` must be valid for `cap` writes, or may be null
// when `cap` is zero.
size_t lcr_cft_receiver_missing(const LcrCftReceiver *r, uint32_t *out, size_t cap);

// The manifest's filename, if there is a manifest and its name is safe to use as one.
// Null otherwise. Release with `lcr_string_free`.
//
// # Safety
//
// `r` must be null or a live handle.
char *lcr_cft_receiver_filename(const LcrCftReceiver *r);

// Bytes the manifest promised, for a progress readout. Zero before the manifest arrives.
//
// # Safety
//
// `r` must be null or a live handle.
uint64_t lcr_cft_receiver_original_size(const LcrCftReceiver *r);

// Reassemble, decompress, and verify against the manifest's hash. Release `out` with
// `lcr_bytes_free`.
//
// Succeeds only when the digest matches, so a caller cannot hand a user a file that did
// not survive the trip.
//
// # Safety
//
// `r` must be a live handle and `out` valid for writes.
LcrStatus lcr_cft_receiver_finish(const LcrCftReceiver *r, LcrBytes *out);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* LCR_H */
