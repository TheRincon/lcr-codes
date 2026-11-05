# ZALT: `max_decoded_size` does not bound a multi-frame stream

A note for fixing this in [ZALT](https://github.com/TheRincon/zalt), found while moving
`lcr-cft` onto `zalt-codec`. Everything below was checked against the published
`zalt-codec` 0.1.0 and `zalt-embed` 0.1.0.

## The gap

`DecodeLimits::max_decoded_size` applies to each frame separately, and the count restarts
at every frame. A stream of concatenated frames can therefore produce far more than the
limit, even though each frame obeys it.

A 43 KB stream of 1,024 frames decodes to **1 GiB under a 1 MiB limit**:

| Input | Limit | Result |
|---|---|---|
| One frame of exactly 1 MiB | 1 MiB | `Ok(1048576)`, correct |
| One frame of 1 MiB + 1 byte | 1 MiB | `Err(TooLarge)`, correct |
| 1,024 of those 1 MiB frames (43,008 bytes) | 1 MiB | **`Ok(1073741824)`** |

The result is the same through `zalt_codec::decode::decompress`, the streaming `Decoder`,
and `zalt_embed::decompress`. Stopping this kind of decompression bomb is the usual reason
to set an output limit, and RFC 8878 allows concatenated frames, so a hostile sender can
do this with a valid stream.

The field's doc comment does say "Reject *frames* whose declared or actual output exceeds
this", so the code matches its wording. But nothing warns that the limit doesn't cover the
whole stream, and `decompress(input, limits)` reads as though it does.

## Reproduction

```toml
[dependencies]
zalt-codec = "0.1.0"
zalt-embed = "0.1.0"
```

```rust
use zalt_codec::decode::decompress;
use zalt_codec::encode::{EncodeParams, Strategy, compress};
use zalt_codec::params::DecodeLimits;

fn main() {
    const LIMIT: u64 = 1 << 20;
    let limits = DecodeLimits { max_window_log: 23, max_decoded_size: Some(LIMIT) };
    let params = EncodeParams { level: 19, strategy: Strategy::Compress, ..Default::default() };

    let frame = compress(&vec![0u8; LIMIT as usize], &params).unwrap();
    let stream: Vec<u8> = frame.iter().copied().cycle().take(frame.len() * 1024).collect();

    // Expected: Err(TooLarge). Actual: Ok(1073741824).
    println!("{:?}", decompress(&stream, limits).map(|v| v.len()));

    let mut policy = zalt_embed::Policy::untrusted();
    policy.limits.max_decoded_size = Some(LIMIT);
    // Same result.
    println!("{:?}", zalt_embed::decompress(&stream, policy).map(|v| v.len()));
}
```

## Root cause

All in `zalt-codec/src/decode.rs`:

- `self.decoded` is set back to 0 at each new frame header (line 460).
- `check_limits` (line 788) and `block_output_cap` (line 723) compare the limit against
  that per-frame count.
- The header check in `src/frame.rs` (line 168) compares each frame's declared size against
  the whole limit, ignoring what earlier frames already produced.
- `decoded_len()` also reports only the current frame, so a caller draining its output
  buffer has no running total to read.

## What is affected

**Affected:**
- `zalt_codec::decode::decompress` and `decompress_with`.
- `zalt_codec::Decoder`, when the caller keeps feeding it after `FrameComplete`.
- `zalt_embed::decompress` and `zalt_embed::Decoder`, when a limit is set.
- `zalt-recipe`: `narrow` passes on a `max_decoded_size` that looks like a ceiling for the
  whole input. I read this code but didn't run it.

**Not affected, from reading the code:**
- `zalt-archive`: `decode_exactly` in `frames.rs` stops at the first `FrameComplete` and
  checks the declared size against the seek table.

**Separate but related:**
- `Policy::untrusted()` sets `max_decoded_size: None`, so by default the one-shot
  `zalt_embed::decompress` has no output ceiling at all, even for a single frame. As far as
  I can tell from `settle_lease`, `total_memory` only covers the decoder's own footprint,
  not the `Vec` the output goes into. If that's intended, the docs for `untrusted()` should
  say so, since its name suggests it is safe for hostile input.

## What a fix needs

The main decision is whether the field's meaning changes. The options I can see:

1. **Make `max_decoded_size` cover the whole stream.** Keep a running total across frames
   and check it in all three places above. This fits how the API reads, and is what I'd
   choose. `reset()` must clear the total, while moving to the next frame must not.
2. **Add a separate stream-wide limit**, and document `max_decoded_size` as per-frame. This
   changes no existing behaviour, but every caller has to know to set the new field.

Either way, these should hold:

- The header check should compare a frame's declared size against what remains of the
  limit, so an oversized frame is refused before any of it is decoded.
- `decode_exactly` in `zalt-archive` and `narrow` in `zalt-recipe` need rechecking against
  the new meaning.
- A running total should be readable, because a caller that drains its output buffer can't
  count it from the buffer's length.
- `DecodeLimits` is `Copy` and crosses the FFI and WIT boundaries, so adding a field or
  changing an error is an interface change. `Error::code` values must stay as they are.

Tests to add:

- The 1,024-frame stream above returns `TooLarge`, through the one-shot and streaming
  decoders and through `zalt-embed`.
- Two frames that together land exactly on the limit succeed; one more byte fails.
- The second frame's declared size alone is enough to refuse it, with no block decoded.
- Skippable frames between data frames don't count towards the limit.
- `reset()` lets a decoder start a new stream with the full limit again.

## How `lcr-cft` handles it for now

`lcr-cft` checks the total itself after every decoder call and refuses anything past the
size the manifest declares. Each call stops at a frame boundary and each frame is capped at
the limit, so memory stays within about twice the declared size. A regression test there
covers the concatenated case. That check can stay after a ZALT fix.

## Also noticed

Not part of this gap, and possibly intentional:

`compress_default` and `EncodeParams::default()` use `Strategy::Stored`, so they don't
compress. 180,000 bytes of repetitive text came out as 180,016 bytes, against 96 bytes with
`Strategy::Compress`. The README says `zalt c` defaults to level 3, so a library caller could
reasonably expect the same from `compress_default`.
