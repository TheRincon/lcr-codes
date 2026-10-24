//! Fountain repair symbols, for carriers with no way back (CFT.md §21).
//!
//! A camera pointed at a screen cannot say "chunk 41 again". The plain transfer shows
//! every chunk in turn and the receiver waits for the loop to come round to whatever it
//! missed — and a symbol it misses on the second pass costs a third. With repair symbols
//! the sender never repeats itself: after one pass of the source chunks it shows an
//! endless stream of new symbols, each the XOR of a pseudo-random set of chunks, and *any*
//! slightly-more-than-`K` of them reconstruct the file. Which ones were missed stops
//! mattering.
//!
//! The code is LT (Luby, 2002) in systematic form:
//!
//! * symbol ids `0..K` are the source chunks, sent as ordinary [`Chunk`] messages, so a
//!   receiver that predates this module still reads them;
//! * ids `K..` are [`Repair`] messages, whose chunk set is derived from the id alone by
//!   [`Degrees::neighbours`], so it never travels on the wire.
//!
//! **Not RaptorQ.** RFC 6330 decodes with less overhead, but it is covered by patent
//! declarations, and the LCR Code spec's royalty-free pledge cannot extend over somebody
//! else's patents. LT's own patents have expired.
//!
//! **Integer arithmetic only.** Sender and receiver must derive bit-identical chunk sets,
//! and they are routinely different builds — the CLI natively, the browser through wasm.
//! A degree table computed with `ln` and `sqrt` in floating point can differ in the last
//! place between platforms' maths libraries, and one flipped threshold is a symbol the
//! receiver XORs into the wrong chunks. Everything here is `u64`/`u128`.

use crate::manifest::has_duplicate_keys;
use crate::{CftError, Limits, Reassembler, Status, Transfer};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use lcr_core::cbor::Value;

/// Wire value of the only scheme defined: systematic LT with [`Degrees`].
pub const SCHEME_LT: u8 = 1;

/// In [`Fountain::frame`]'s schedule, one frame in this many re-shows the manifest, so a
/// receiver that starts watching mid-stream is never long without one.
pub const MANIFEST_EVERY: u64 = 8;

mod key {
    pub const TRANSFER_ID: i64 = 0;
    pub const ID: i64 = 1;
    pub const TOTAL: i64 = 2;
    pub const PAYLOAD: i64 = 3;
    /// Present only in a repair symbol, which is what tells it from a [`super::Chunk`].
    pub const SCHEME: i64 = 4;
}

/// One repair symbol: the XOR of the source chunks [`Degrees::neighbours`] names for `id`.
///
/// Every source chunk is zero-padded to the manifest's `chunk_size` before the XOR, so
/// `payload` is always exactly `chunk_size` bytes, the short final chunk included.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Repair {
    /// Which transfer this belongs to.
    pub transfer_id: Vec<u8>,
    /// Symbol id, at least `total`: ids below it are the source chunks themselves.
    pub id: u32,
    /// Source chunk count, `K`. Must agree with the manifest.
    pub total: u32,
    /// XOR of the padded source chunks.
    pub payload: Vec<u8>,
}

impl Repair {
    /// Serialise to CBOR: a chunk's keys, plus key 4 naming the scheme.
    pub fn to_vec(&self) -> Vec<u8> {
        Value::Map(alloc::vec![
            (
                Value::from_i64(key::TRANSFER_ID),
                Value::Bytes(self.transfer_id.clone()),
            ),
            (Value::from_i64(key::ID), Value::Uint(self.id as u64)),
            (Value::from_i64(key::TOTAL), Value::Uint(self.total as u64)),
            (
                Value::from_i64(key::PAYLOAD),
                Value::Bytes(self.payload.clone()),
            ),
            (Value::from_i64(key::SCHEME), Value::Uint(SCHEME_LT as u64),),
        ])
        .to_vec()
    }

    /// Parse a repair symbol.
    pub fn from_slice(bytes: &[u8], limits: &Limits) -> Result<Repair, CftError> {
        let value = Value::from_slice(bytes).map_err(|_| CftError::Malformed)?;
        let map = value.as_map().ok_or(CftError::Malformed)?;
        if has_duplicate_keys(map) {
            return Err(CftError::Malformed);
        }
        match value.get_int_key(key::SCHEME) {
            Some(Value::Uint(s)) if *s == SCHEME_LT as u64 => {}
            Some(Value::Uint(s)) => return Err(CftError::Unsupported((*s).min(255) as u8)),
            Some(_) => return Err(CftError::BadField(key::SCHEME as u8)),
            None => return Err(CftError::MissingField(key::SCHEME as u8)),
        }
        let transfer_id = match value.get_int_key(key::TRANSFER_ID) {
            Some(Value::Bytes(b)) if !b.is_empty() => b.clone(),
            Some(_) => return Err(CftError::BadField(key::TRANSFER_ID as u8)),
            None => return Err(CftError::MissingField(key::TRANSFER_ID as u8)),
        };
        let id =
            u32::try_from(uint(&value, key::ID)?).map_err(|_| CftError::BadField(key::ID as u8))?;
        let total = u32::try_from(uint(&value, key::TOTAL)?)
            .map_err(|_| CftError::BadField(key::TOTAL as u8))?;
        // A repair id below `total` would name a source chunk, which travels as a chunk.
        if total == 0 || total > limits.max_chunks || id < total {
            return Err(CftError::ChunkOutOfRange { index: id, total });
        }
        let payload = match value.get_int_key(key::PAYLOAD) {
            Some(Value::Bytes(b)) if b.len() <= limits.max_chunk => b.clone(),
            Some(Value::Bytes(_)) => return Err(CftError::ChunkLength(id)),
            Some(_) => return Err(CftError::BadField(key::PAYLOAD as u8)),
            None => return Err(CftError::MissingField(key::PAYLOAD as u8)),
        };
        Ok(Repair {
            transfer_id,
            id,
            total,
            payload,
        })
    }
}

/// Whether `bytes` is a repair symbol rather than a manifest or a chunk: a CBOR map with
/// a byte-string key 0 (as a chunk has) **and** a key 4 (which no chunk has).
///
/// A receiver that predates this module parses a repair symbol as a chunk and rejects it,
/// because its id is at least the chunk count — so old receivers ignore repair symbols
/// rather than misreading them.
pub fn is_repair(bytes: &[u8]) -> bool {
    let Ok(value) = Value::from_slice(bytes) else {
        return false;
    };
    matches!(value.get_int_key(0), Some(Value::Bytes(_)))
        && value.get_int_key(key::SCHEME).is_some()
}

/// SplitMix64 (Steele, Lea and Flood, 2014): the generator behind every chunk set.
///
/// Chosen because it is three lines, has no state beyond one `u64`, and is designed to be
/// seeded with consecutive integers — exactly what symbol ids are.
#[derive(Clone, Debug)]
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n`, by multiply-and-shift rather than `%`.
    fn below(&mut self, n: u64) -> u64 {
        ((self.next() as u128 * n as u128) >> 64) as u64
    }
}

/// One repair symbol in this many — the last of each group, counting from the first
/// repair id `K` — is **dense**: a random half of all chunks rather than a soliton draw.
///
/// Soliton symbols are what let a receiver that starts from nothing peel its way in, and
/// they are mostly degree 2 or 3. That is wasted on a receiver that watched the source pass
/// and missed only a few chunks: a degree-2 symbol misses its few gaps most of the time.
/// A dense one almost always covers some, and [`crate::Reassembler`] finishes those by
/// elimination once few enough chunks remain. A receiver with too many gaps for that
/// discards dense symbols, so they cost a late joiner a quarter of its frames rather than
/// any of its memory.
pub const DENSE_EVERY: u32 = 8;

/// Up to this many chunks, **every** repair symbol is dense.
///
/// A receiver solves by elimination once at most this many chunks are unknown, so for a
/// transfer this small it can always solve the whole file that way — and against
/// elimination, random dense symbols are close to optimal, needing about `K + 2` in all.
/// Sparse symbols only pay for themselves where peeling has to do the work: at 9 chunks
/// a degree-2 symbol misses a given gap three times in four. Matches the receiver's
/// elimination limit.
pub const ALL_DENSE_UP_TO: u32 = 256;

/// Fixed-point unit of the degree weights.
const UNIT: u128 = 1 << 32;

/// Highest degree ever drawn. The ideal-soliton weight `UNIT / (d(d−1))` is below one unit
/// past here, so the table ends where its weights would all floor to zero anyway.
const MAX_DEGREE: u32 = 65_536;

/// The robust soliton degree distribution (Luby, 2002), in integer form, for one `K`.
///
/// With `S = max(1, ⌊√K⌋)`, spike `M = max(1, ⌊K / S⌋)` and `b` the bit length of `20·S`,
/// the weight of degree `d` in `1..=min(K, 65536)` is `ρ(d) + τ(d)`, each floored on its
/// own:
///
/// ```text
/// ρ(1) = U / K                 ρ(d) = U / (d·(d−1))            for d ≥ 2
/// τ(d) = U·S / (K·d)           for d < M
/// τ(M) = U·S·693·b / (1000·K)
/// τ(d) = 0                     for d > M
/// ```
///
/// with `U = 2³²`. That is Luby's distribution with `R = S` — which is what `c·ln(K/δ)·√K`
/// comes to at his recommended `c ≈ 0.1, δ = 0.05` for the `K` a file transfer meets —
/// and `ln(S/δ)` approximated as `0.693 × b`. The approximation does not need to be good,
/// only deterministic: it is the definition, and both ends use it.
#[derive(Clone, Debug)]
pub struct Degrees {
    k: u32,
    /// `cdf[d − 1]` is the total weight of degrees `1..=d`.
    cdf: Vec<u64>,
}

impl Degrees {
    /// The distribution for `k` source chunks. `k` must be at least 1.
    pub fn new(k: u32) -> Degrees {
        let k = k.max(1);
        let kk = k as u128;
        let s = (isqrt(k as u64).max(1)) as u128;
        let m = (kk / s).max(1);
        let b = (128 - (20 * s).leading_zeros()) as u128;
        let top = k.min(MAX_DEGREE);
        let mut cdf = Vec::with_capacity(top as usize);
        let mut total: u64 = 0;
        for d in 1..=top as u128 {
            let rho = if d == 1 {
                UNIT / kk
            } else {
                UNIT / (d * (d - 1))
            };
            let tau = if d < m {
                UNIT * s / (kk * d)
            } else if d == m {
                UNIT * s * 693 * b / (1000 * kk)
            } else {
                0
            };
            total += (rho + tau) as u64;
            cdf.push(total);
        }
        Degrees { k, cdf }
    }

    /// Source chunk count this table was built for.
    pub fn k(&self) -> u32 {
        self.k
    }

    /// Whether repair symbol `id` is dense (see [`DENSE_EVERY`]).
    pub fn is_dense(&self, id: u32) -> bool {
        id >= self.k
            && (self.k <= ALL_DENSE_UP_TO || (id - self.k) % DENSE_EVERY == DENSE_EVERY - 1)
    }

    /// The source chunks repair symbol `id` combines, in ascending order.
    ///
    /// The generator is SplitMix64 seeded with `id`.
    ///
    /// **Sparse symbols** (most): the first output picks the degree `d` —
    /// `r = ⌊next · Z / 2⁶⁴⌋` for total weight `Z`, and `d` is the smallest degree whose
    /// cumulative weight exceeds `r`. If `d ≥ K` every chunk is in the set. Otherwise each
    /// further output `n` names chunk `⌊n · K / 2⁶⁴⌋`, a repeat is drawn again, and the
    /// first `d` distinct chunks are the set.
    ///
    /// **Dense symbols** ([`Degrees::is_dense`]): outputs are taken as a bit string, least
    /// significant bit first, and chunk `j` is in the set when bit `j` is 1. A set that
    /// comes out empty is instead the single chunk `⌊next · K / 2⁶⁴⌋`.
    pub fn neighbours(&self, id: u32) -> Vec<u32> {
        let mut rng = SplitMix64(id as u64);
        if self.is_dense(id) {
            let mut set = Vec::new();
            let mut word = 0;
            for j in 0..self.k {
                if j % 64 == 0 {
                    word = rng.next();
                }
                if word >> (j % 64) & 1 == 1 {
                    set.push(j);
                }
            }
            if set.is_empty() {
                set.push(rng.below(self.k as u64) as u32);
            }
            return set;
        }
        let z = *self.cdf.last().unwrap_or(&1);
        let r = ((rng.next() as u128 * z as u128) >> 64) as u64;
        let d = self.cdf.partition_point(|&c| c <= r) as u32 + 1;
        if d >= self.k {
            return (0..self.k).collect();
        }
        let mut set = BTreeSet::new();
        while set.len() < d as usize {
            set.insert(rng.below(self.k as u64) as u32);
        }
        set.into_iter().collect()
    }
}

/// `⌊√n⌋`, exactly.
fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    // Newton's method from an upper bound; integer-only, so it agrees on every platform.
    let mut x = 1u64 << (64 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// XOR `src` into the front of `dst` — a short chunk counts as zero-padded.
pub(crate) fn xor_into(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d ^= s;
    }
}

/// The sending half: a prepared [`Transfer`] that can produce any symbol id on demand.
#[derive(Clone, Debug)]
pub struct Fountain {
    transfer: Transfer,
    degrees: Degrees,
}

impl Fountain {
    /// Wrap a transfer from [`crate::split`].
    ///
    /// An inline transfer — the whole stream inside the manifest — has nothing to repair:
    /// every symbol it produces is that manifest.
    pub fn new(transfer: Transfer) -> Fountain {
        let degrees = Degrees::new(transfer.manifest.total_chunks);
        Fountain { transfer, degrees }
    }

    /// The transfer being sent.
    pub fn transfer(&self) -> &Transfer {
        &self.transfer
    }

    /// Source chunk count, `K`.
    pub fn source_count(&self) -> u32 {
        self.transfer.manifest.total_chunks
    }

    /// The manifest message.
    pub fn manifest(&self) -> Vec<u8> {
        self.transfer.manifest.to_vec()
    }

    /// Symbol `id` as a CFT message: chunk `id` below `K`, a [`Repair`] from `K` up.
    pub fn symbol(&self, id: u32) -> Vec<u8> {
        if self.transfer.chunks.is_empty() {
            return self.manifest();
        }
        match self.transfer.chunks.get(id as usize) {
            Some(chunk) => chunk.to_vec(),
            None => self.repair(id).to_vec(),
        }
    }

    /// Repair symbol `id`, which must be at least `K`.
    pub fn repair(&self, id: u32) -> Repair {
        let m = &self.transfer.manifest;
        let mut payload = alloc::vec![0u8; m.chunk_size as usize];
        for j in self.degrees.neighbours(id) {
            xor_into(&mut payload, &self.transfer.chunks[j as usize].payload);
        }
        Repair {
            transfer_id: m.transfer_id.clone(),
            id,
            total: m.total_chunks,
            payload,
        }
    }

    /// The message for frame `n` of the recommended carousel: the manifest on every
    /// [`MANIFEST_EVERY`]th frame, and otherwise symbol ids in order — the source chunks
    /// once, then repair symbols without end.
    ///
    /// A receiver watching from the start needs `K` frames plus whatever it dropped plus a
    /// few percent; one that arrives late finds the stream already repairing and needs
    /// about the same from wherever it joined.
    pub fn frame(&self, n: u64) -> Vec<u8> {
        if n.is_multiple_of(MANIFEST_EVERY) {
            return self.manifest();
        }
        let id = n - n / MANIFEST_EVERY - 1;
        self.symbol(id.min(u32::MAX as u64) as u32)
    }
}

// ---------------------------------------------------------------- receiving

/// Most unknown chunks a [`Reassembler`] will solve for by elimination at once. At 256 the
/// worst case is about 33,000 chunk-sized XORs — tens of milliseconds in a browser.
///
/// The same number as [`ALL_DENSE_UP_TO`], and it has to be: a sender makes every repair
/// symbol dense up to that size on the promise that a receiver can always eliminate.
const SOLVE_MAX: usize = ALL_DENSE_UP_TO as usize;

/// A repair symbol partway through peeling.
#[derive(Clone, Debug)]
struct Pending {
    /// The chunks it still combines that are not yet known.
    unknown: Vec<u32>,
    /// Its payload with every known chunk XORed back out.
    payload: Vec<u8>,
}

/// The fountain decoder's share of a [`Reassembler`].
#[derive(Clone, Debug, Default)]
pub(crate) struct Peel {
    /// Repair symbols that arrived before the manifest did.
    orphan_repairs: Vec<Repair>,
    /// Repair symbols still combining two or more unknown chunks.
    pending: Vec<Option<Pending>>,
    /// How many entries of `pending` are still live.
    live: usize,
    /// For each chunk, the `pending` entries that still include it.
    by_chunk: Vec<Vec<u32>>,
    /// Repair ids already taken: a camera reads the same symbol on several frames.
    seen: BTreeSet<u32>,
    /// Built on the first repair symbol, so a plain transfer never pays for it.
    degrees: Option<Degrees>,
}

impl Peel {
    /// Size the per-chunk index once the manifest says how many chunks there are.
    pub(crate) fn start(&mut self, total: u32) {
        self.by_chunk = alloc::vec![Vec::new(); total as usize];
    }

    /// Repair symbols held for want of a manifest.
    pub(crate) fn orphans(&self) -> usize {
        self.orphan_repairs.len()
    }
}

/// The receiving half of a fountain transfer, decoded by *peeling*: every chunk a repair
/// symbol combines that is already known is XORed out of it, and once one unknown chunk is
/// left the symbol *is* that chunk — which may in turn reduce other held symbols to one.
/// Elimination finishes what peeling leaves (see [`Reassembler::accept_repair`]).
impl Reassembler {
    /// Repair symbols held that have not yet resolved to a chunk.
    ///
    /// Grows while a fountain transfer is short of chunks and collapses as they arrive,
    /// often several at once: one recovered chunk can reduce a held symbol to one unknown,
    /// which recovers another.
    pub fn pending(&self) -> usize {
        self.peel.live
    }

    /// Distinct repair symbols taken so far, resolved or not.
    pub fn repairs_seen(&self) -> usize {
        self.peel.seen.len()
    }

    /// Re-offer repair symbols that arrived before the manifest.
    pub(crate) fn replay_repairs(&mut self) -> Result<(), CftError> {
        for repair in core::mem::take(&mut self.peel.orphan_repairs) {
            self.accept_repair(repair)?;
        }
        Ok(())
    }

    /// Take a fountain repair symbol (CFT.md §21).
    ///
    /// After peeling, finishes by Gaussian elimination once peeling has stalled but the
    /// held symbols already determine every missing chunk. Peeling alone is what makes LT
    /// cheap, and also why plain LT needs 1.3–1.6× `K` symbols at the sizes a file transfer
    /// meets: it waits for some symbol to come down to one unknown, when the system as a
    /// whole was solvable long before. Elimination over GF(2) solves it as soon as the held
    /// symbols have full rank. It costs O(u²) chunk-sized XORs for `u` unknowns, so it only
    /// runs once `u` is at most 256 — all of a small transfer, and the tail of a large one,
    /// where peeling is slowest anyway.
    pub fn accept_repair(&mut self, repair: Repair) -> Result<Status, CftError> {
        let Some(manifest) = &self.manifest else {
            // Held for the same reason an early chunk is: nothing to check it against.
            // Once per id, as for chunks.
            let held = self
                .peel
                .orphan_repairs
                .iter()
                .any(|o| o.id == repair.id && o.transfer_id == repair.transfer_id);
            if !held && self.peel.orphan_repairs.len() < self.limits.max_chunks as usize {
                self.peel.orphan_repairs.push(repair);
            }
            return Ok(self.status());
        };
        if repair.transfer_id != manifest.transfer_id {
            return Err(CftError::WrongTransfer);
        }
        if repair.total != manifest.total_chunks {
            return Err(CftError::ChunkOutOfRange {
                index: repair.id,
                total: manifest.total_chunks,
            });
        }
        if repair.payload.len() != manifest.chunk_size as usize {
            return Err(CftError::ChunkLength(repair.id));
        }
        let total = manifest.total_chunks;
        // Already whole, or this exact symbol was read on an earlier frame.
        if self.have == total || !self.peel.seen.insert(repair.id) {
            return Ok(self.status());
        }
        // A sender cannot need more than a few times `K` repair symbols held at once; past
        // that it is flooding, and every held symbol costs a whole chunk of memory.
        if self.peel.live >= 2 * total as usize + 64 {
            return Ok(self.status());
        }

        let degrees = self.peel.degrees.get_or_insert_with(|| Degrees::new(total));
        let dense = degrees.is_dense(repair.id);
        let neighbours = degrees.neighbours(repair.id);
        let unknown: Vec<u32> = neighbours
            .iter()
            .copied()
            .filter(|&j| self.slots[j as usize].is_none())
            .collect();
        // A dense symbol can only ever be finished by elimination, and with this many gaps
        // elimination will not run: holding it would cost a chunk of memory and a XOR every
        // time any of its hundreds of chunks landed, for nothing.
        if dense && unknown.len() > SOLVE_MAX {
            return Ok(self.status());
        }
        let mut payload = repair.payload;
        for j in neighbours {
            if let Some(known) = &self.slots[j as usize] {
                xor_into(&mut payload, known);
            }
        }
        match unknown.as_slice() {
            [] => {}
            &[only] => self.learn_and_peel(only, payload)?,
            _ => {
                let pid = self.peel.pending.len() as u32;
                for &j in &unknown {
                    self.peel.by_chunk[j as usize].push(pid);
                }
                self.peel.pending.push(Some(Pending { unknown, payload }));
                self.peel.live += 1;
            }
        }
        self.solve()?;
        Ok(self.status())
    }

    /// Gaussian elimination over the held symbols, when it can finish the transfer.
    fn solve(&mut self) -> Result<(), CftError> {
        let Some(m) = &self.manifest else {
            return Ok(());
        };
        let total = m.total_chunks as usize;
        let u = total - self.have as usize;
        if u == 0 || u > SOLVE_MAX || self.peel.live < u {
            return Ok(());
        }
        let cols: Vec<u32> = (0..total as u32)
            .filter(|&j| self.slots[j as usize].is_none())
            .collect();
        let words = u.div_ceil(64);
        let pending = &self.peel.pending;
        let bits_of = |p: &Pending| {
            let mut b = alloc::vec![0u64; words];
            for &j in &p.unknown {
                // Every pending symbol's unknowns are, by construction, still unknown.
                if let Ok(c) = cols.binary_search(&j) {
                    b[c / 64] |= 1 << (c % 64);
                }
            }
            b
        };

        // Rank first, on the coefficient bits alone — cheap, and most calls end here.
        // The rows that become pivots are a basis, so only they need their payloads.
        let mut work: Vec<(Vec<u64>, usize)> = pending
            .iter()
            .enumerate()
            .filter_map(|(p, q)| q.as_ref().map(|q| (bits_of(q), p)))
            .collect();
        // Every column must find a pivot, so row `c` is always the next one free.
        let mut basis = Vec::with_capacity(u);
        for c in 0..u {
            let (w, bit) = (c / 64, 1u64 << (c % 64));
            let Some(r) = (c..work.len()).find(|&r| work[r].0[w] & bit != 0) else {
                return Ok(()); // not yet full rank; try again on the next symbol
            };
            work.swap(c, r);
            let pivot = work[c].0.clone();
            for row in work.iter_mut().skip(c + 1) {
                if row.0[w] & bit != 0 {
                    for (a, b) in row.0.iter_mut().zip(&pivot) {
                        *a ^= b;
                    }
                }
            }
            basis.push(work[c].1);
        }

        // Full rank: Gauss–Jordan over just the basis rows, payloads included.
        let mut sys: Vec<(Vec<u64>, Vec<u8>)> = basis
            .iter()
            .filter_map(|&p| pending[p].as_ref().map(|q| (bits_of(q), q.payload.clone())))
            .collect();
        for c in 0..u {
            let (w, bit) = (c / 64, 1u64 << (c % 64));
            let Some(r) = (c..sys.len()).find(|&r| sys[r].0[w] & bit != 0) else {
                return Ok(());
            };
            sys.swap(c, r);
            let (before, rest) = sys.split_at_mut(c);
            let Some((pivot, after)) = rest.split_first_mut() else {
                return Ok(());
            };
            for row in before.iter_mut().chain(after.iter_mut()) {
                if row.0[w] & bit != 0 {
                    for (a, b) in row.0.iter_mut().zip(&pivot.0) {
                        *a ^= b;
                    }
                    xor_into(&mut row.1, &pivot.1);
                }
            }
        }
        // Row c now holds exactly chunk cols[c]. Recording each retires every pending
        // symbol as its chunks land.
        for (c, (_, payload)) in sys.into_iter().enumerate() {
            self.learn_and_peel(cols[c], payload)?;
        }
        Ok(())
    }

    /// Record chunk `index`, then peel: XOR it out of every held repair symbol that
    /// includes it, and record any symbol that comes down to a single unknown as that
    /// chunk — repeating until nothing more resolves.
    pub(crate) fn learn_and_peel(&mut self, index: u32, bytes: Vec<u8>) -> Result<(), CftError> {
        let mut queue = alloc::vec![(index, bytes)];
        while let Some((i, bytes)) = queue.pop() {
            if !self.store(i, bytes)? {
                continue;
            }
            for pid in core::mem::take(&mut self.peel.by_chunk[i as usize]) {
                let Some(p) = self.peel.pending[pid as usize].as_mut() else {
                    continue;
                };
                let Some(pos) = p.unknown.iter().position(|&j| j == i) else {
                    continue;
                };
                p.unknown.swap_remove(pos);
                if let Some(known) = &self.slots[i as usize] {
                    xor_into(&mut p.payload, known);
                }
                if p.unknown.len() <= 1 {
                    if let Some(Pending { unknown, payload }) =
                        self.peel.pending[pid as usize].take()
                        && let &[only] = unknown.as_slice()
                    {
                        queue.push((only, payload));
                    }
                    self.peel.live -= 1;
                }
            }
        }
        Ok(())
    }
}

fn uint(value: &Value, k: i64) -> Result<u64, CftError> {
    match value.get_int_key(k) {
        Some(Value::Uint(u)) => Ok(*u),
        Some(_) => Err(CftError::BadField(k as u8)),
        None => Err(CftError::MissingField(k as u8)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isqrt_is_exact() {
        for n in 0..10_000u64 {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "n={n} r={r}");
        }
        assert_eq!(isqrt(u64::MAX), u32::MAX as u64);
    }

    #[test]
    fn neighbours_are_distinct_in_range_and_repeatable() {
        for k in [1u32, 2, 3, 10, 97, 1000] {
            let d = Degrees::new(k);
            for id in k..k + 200 {
                let n = d.neighbours(id);
                assert!(!n.is_empty() && n.len() <= k as usize);
                assert!(n.windows(2).all(|w| w[0] < w[1]), "sorted and distinct");
                assert!(n.iter().all(|&j| j < k));
                assert_eq!(n, Degrees::new(k).neighbours(id));
            }
        }
    }

    /// Pinned so a change to the generator or the table — which would make every
    /// existing sender and receiver disagree — cannot pass unnoticed.
    #[test]
    fn neighbours_are_pinned() {
        // Sparse: past `ALL_DENSE_UP_TO`, and off the one-in-eight dense slot.
        let d = Degrees::new(1000);
        assert!((1000..1004).all(|id| !d.is_dense(id)) && d.is_dense(1000 + DENSE_EVERY - 1));
        let got: Vec<Vec<u32>> = (1000..1004).map(|id| d.neighbours(id)).collect();
        assert_eq!(got, PINNED_SPARSE_1000);
        // Dense: every repair symbol of a small transfer, about half its chunks.
        let d = Degrees::new(100);
        assert!((100..110).all(|id| d.is_dense(id)) && !d.is_dense(99));
        let dense = d.neighbours(100);
        assert_eq!((dense.len(), dense.iter().sum::<u32>()), PINNED_DENSE_100);
    }

    const PINNED_SPARSE_1000: [&[u32]; 4] = [
        &[773, 814],
        &[228, 229],
        &[
            23, 53, 57, 75, 104, 169, 172, 228, 291, 379, 380, 513, 516, 525, 547, 631, 651, 761,
            762, 763, 766, 778, 827, 840, 889, 910, 915, 920, 923, 953, 958, 993,
        ],
        &[176, 416, 534, 984],
    ];
    const PINNED_DENSE_100: (usize, u32) = (51, 2576);

    #[test]
    fn mean_degree_is_soliton_like() {
        // Robust soliton's mean degree grows like ln K; far outside that and the table is
        // wrong, whatever the decode tests say.
        let d = Degrees::new(1000);
        let sparse: Vec<u32> = (1000..21_000).filter(|&id| !d.is_dense(id)).collect();
        let mean: f64 = sparse
            .iter()
            .map(|&id| d.neighbours(id).len() as f64)
            .sum::<f64>()
            / sparse.len() as f64;
        assert!((4.0..20.0).contains(&mean), "mean degree {mean}");
    }
}
