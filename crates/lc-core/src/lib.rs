//! # lc-core
//!
//! Format-defining primitives for LC Code (spec v0.4): symbol geometry, the module-role
//! layout, the palette and colour maths, masking, interleaving, LDPC error correction,
//! the BCH-protected format region, and the payload container.
//!
//! This crate is `no_std + alloc` and contains no I/O, no crypto and no floating-point
//! colour conversion in the hot decode path beyond what soft demodulation requires.
//!
//! ## Where this implementation pins the spec's `[TBD]`s
//!
//! The v0.4 draft leaves a number of wire-format decisions open. A reference
//! implementation cannot; every one of them is pinned here and documented in
//! `docs/decisions.md`. The short list:
//!
//! | Spec item | Pinned as |
//! |---|---|
//! | §7 BCH(30,15) generator | shortened BCH(31,16), `g(x) = 0x8FAF` ([`bch`]) |
//! | §8.2 LDPC base graphs | `n_b = 60` QC base graphs, [`ecc::ldpc::tables`] |
//! | §8.3 Interleaver | golden-ratio affine permutation ([`interleave`]) |
//! | §6.1 Capacity table | computed by [`capacity`] |
//! | §6.2 Interior alignment markers | QR-style coordinate list ([`layout`]) |
//! | §4.4 8-colour palette | derived in CIELCh, pinned in [`palette`] |
//! | §6.2 Palette anchor placement | ring-1 outer arms ([`layout`]) |
//!
//! ## Layering
//!
//! ```text
//! bits ── crc ── frame ─────────────┐  §9  container
//! cbor ── json ────────────────────┘
//! version/profile ── layout ───────┐  §6  geometry
//! palette ── color ────────────────┤  §4
//! mask ── interleave ── ecc::ldpc ─┤  §8
//! bch ── format ───────────────────┘  §7
//! ```

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod bch;
pub mod bits;
pub mod border;
pub mod capacity;
pub mod cbor;
pub mod color;
pub mod crc;
pub mod ecc;
pub mod error;
pub mod format;
pub mod frame;
pub mod grid;
pub mod interleave;
pub mod json;
pub mod layout;
pub mod mask;
pub mod modulate;
pub mod packet;
pub mod palette;
pub mod symbol;
pub mod version;

pub use error::LcError;
pub use format::FormatInfo;
pub use frame::{Compression, ContentType, Frame};
pub use grid::{ModuleGrid, ModuleRole};
pub use packet::{Packet, VerifyFailure};
pub use palette::Palette;
pub use symbol::Symbol;
pub use version::{EccLevel, Profile, Version};
