//! Fountain transfers (CFT.md §21): a one-way carousel that a receiver can join at any
//! point and leave as soon as it holds enough, whatever it missed.

use lcr_cft::fountain::MANIFEST_EVERY;
use lcr_cft::{CftError, Compression, Fountain, Reassembler, Repair, Status, split};

const ID: &[u8] = b"\x8f\x13\xa9\x2c";

/// Incompressible, so the chunk count is what the size says it is.
fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as u8
        })
        .collect()
}

fn fountain(len: usize, chunk: u32) -> (Vec<u8>, Fountain) {
    let data = noise(len, len as u32);
    let t = split(
        &data,
        Some("noise.bin".into()),
        ID.to_vec(),
        Compression::None,
        chunk,
    )
    .unwrap();
    (data, Fountain::new(t))
}

/// A deterministic coin with probability `p`, so a failure reproduces.
struct Coin(u64);
impl Coin {
    fn flip(&mut self, p: f64) -> bool {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) < p
    }
}

/// Run the carousel from `start`, dropping each frame with probability `loss`, until the
/// file is whole. Returns the frames it took and the data frames actually received.
fn watch(f: &Fountain, start: u64, loss: f64, seed: u64) -> (Reassembler, u64, u64) {
    let mut r = Reassembler::default();
    let mut coin = Coin(seed | 1);
    let mut received = 0;
    for n in start.. {
        if coin.flip(loss) {
            continue;
        }
        let msg = f.frame(n);
        if !n.is_multiple_of(MANIFEST_EVERY) {
            received += 1;
        }
        if r.accept(&msg).unwrap() == Status::Complete {
            return (r, n - start + 1, received);
        }
        assert!(
            n - start < 40 * f.source_count() as u64 + 1000,
            "never completed"
        );
    }
    unreachable!()
}

#[test]
fn a_lossless_watch_finishes_in_one_pass() {
    let (data, f) = fountain(50_000, 500);
    let (r, _, received) = watch(&f, 0, 0.0, 1);
    // Systematic: with nothing dropped the source chunks alone complete it.
    assert_eq!(received, f.source_count() as u64);
    assert_eq!(r.finish().unwrap(), data);
    assert_eq!(r.repairs_seen(), 0);
}

/// The point of the exercise. Against a plain carousel, a receiver dropping a fraction of
/// frames waits for the loop to bring each missed chunk round again; here any symbols do.
#[test]
fn heavy_loss_costs_little_more_than_the_loss() {
    for (k_bytes, chunk) in [(20_000, 500), (200_000, 1000)] {
        let (data, f) = fountain(k_bytes, chunk);
        let k = f.source_count() as f64;
        for loss in [0.1, 0.3, 0.5] {
            let mut worst: f64 = 0.0;
            for seed in 1..=5u64 {
                let (r, _, received) = watch(&f, 0, loss, seed * 0x9E37);
                assert_eq!(r.finish().unwrap(), data);
                worst = worst.max(received as f64 / k);
            }
            // Peeling LT at these sizes needs roughly 1.1–1.4 K symbols. Generous, so the
            // test pins "it works" rather than a figure that moves with the seed.
            assert!(worst < 1.8, "K={k} loss={loss}: needed {worst:.2}·K");
        }
    }
}

/// Someone who starts watching after the source pass has gone by sees only repair
/// symbols, and must still finish.
#[test]
fn a_late_joiner_finishes_from_repair_symbols_alone() {
    let (data, f) = fountain(60_000, 600);
    let k = f.source_count() as u64;
    let start = 2 * k; // well past the systematic pass
    let (r, _, received) = watch(&f, start, 0.2, 7);
    assert_eq!(r.finish().unwrap(), data);
    assert!(
        r.repairs_seen() as u64 >= k,
        "fewer than K symbols cannot cover K chunks"
    );
    assert!(received < 2 * k, "needed {received} symbols for K={k}");
}

/// The same symbol read on several frames must count once, or the receiver would think it
/// held more than it does.
#[test]
fn repeated_reads_of_one_symbol_count_once() {
    let (_, f) = fountain(30_000, 500);
    let mut r = Reassembler::default();
    r.accept(&f.manifest()).unwrap();
    let k = f.source_count();
    for _ in 0..5 {
        r.accept(&f.symbol(k + 3)).unwrap();
    }
    assert_eq!(r.repairs_seen(), 1);
    assert!(r.pending() <= 1);
}

/// Before the manifest, every read is held — and a camera reads each symbol on several
/// frames. Held once per symbol, or memory grows with the time spent waiting.
#[test]
fn repeated_reads_before_the_manifest_are_held_once() {
    let (_, f) = fountain(30_000, 500);
    let k = f.source_count();
    let mut r = Reassembler::default();
    for _ in 0..10 {
        r.accept(&f.symbol(2)).unwrap();
        r.accept(&f.symbol(k + 5)).unwrap();
    }
    assert_eq!(r.status(), Status::AwaitingManifest { held: 2 });
}

#[test]
fn repair_symbols_before_the_manifest_are_held() {
    let (data, f) = fountain(30_000, 500);
    let k = f.source_count();
    let mut r = Reassembler::default();
    // Every repair symbol first, the manifest last.
    for id in k..k * 3 {
        r.accept(&f.symbol(id)).unwrap();
    }
    assert!(matches!(r.status(), Status::AwaitingManifest { held } if held > 0));
    assert_eq!(r.accept(&f.manifest()).unwrap(), Status::Complete);
    assert_eq!(r.finish().unwrap(), data);
}

/// The final chunk is short, and a repair symbol carries it zero-padded to `chunk_size`.
/// Recovered through repair, the padding must be stripped or the stream is too long.
#[test]
fn a_short_final_chunk_is_recovered_through_repair() {
    let (data, f) = fountain(10_250, 500); // 20 full chunks and one of 250 bytes
    let k = f.source_count();
    let last = k - 1;
    let mut r = Reassembler::default();
    r.accept(&f.manifest()).unwrap();
    for id in 0..last {
        r.accept(&f.symbol(id)).unwrap();
    }
    // Only the last chunk is missing; the first repair symbol that includes it supplies it.
    let mut id = k;
    while r.status() != Status::Complete {
        r.accept(&f.symbol(id)).unwrap();
        id += 1;
    }
    assert_eq!(r.finish().unwrap(), data);
}

#[test]
fn a_repair_symbol_of_the_wrong_length_is_rejected() {
    let (_, f) = fountain(30_000, 500);
    let k = f.source_count();
    let mut bad = f.repair(k + 1);
    bad.payload.pop();
    let mut r = Reassembler::default();
    r.accept(&f.manifest()).unwrap();
    assert_eq!(r.accept(&bad.to_vec()), Err(CftError::ChunkLength(k + 1)));
}

#[test]
fn a_repair_symbol_from_another_transfer_is_rejected() {
    let (_, f) = fountain(30_000, 500);
    let k = f.source_count();
    let other = Repair {
        transfer_id: b"nope".to_vec(),
        ..f.repair(k)
    };
    let mut r = Reassembler::default();
    r.accept(&f.manifest()).unwrap();
    assert_eq!(r.accept(&other.to_vec()), Err(CftError::WrongTransfer));
}

/// A receiver that predates repair symbols reads one as a chunk and rejects it as out of
/// range — it must never take one for a chunk of the file.
#[test]
fn an_old_style_parse_rejects_a_repair_symbol() {
    use lcr_cft::Limits;
    use lcr_cft::chunk::parse;
    let (_, f) = fountain(30_000, 500);
    let k = f.source_count();
    assert!(matches!(
        parse(&f.symbol(k + 2), &Limits::default()),
        Err(CftError::ChunkOutOfRange { .. })
    ));
}

/// Wire round trip of the message itself.
#[test]
fn a_repair_symbol_round_trips() {
    let (_, f) = fountain(30_000, 500);
    let k = f.source_count();
    let r = f.repair(k + 9);
    assert_eq!(
        Repair::from_slice(&r.to_vec(), &lcr_cft::Limits::default()).unwrap(),
        r
    );
}

/// Overhead survey: `cargo test -p lcr-cft --release --test fountain survey -- --ignored --nocapture`.
#[test]
#[ignore]
fn survey() {
    for k in [10u32, 40, 100, 300, 1000, 3000] {
        let (_, f) = fountain(k as usize * 200, 200);
        let kk = f.source_count();
        // Repair-only (late joiner, no loss) isolates the code's own overhead.
        let mut v: Vec<f64> = (0..30u64)
            .map(|s| {
                let mut r = Reassembler::default();
                r.accept(&f.manifest()).unwrap();
                let base = kk + (s as u32) * 50_000;
                let mut n = 0u32;
                while r.accept(&f.symbol(base + n)).unwrap() != Status::Complete {
                    n += 1;
                }
                (n + 1) as f64 / kk as f64
            })
            .collect();
        v.sort_by(f64::total_cmp);
        println!(
            "K={kk:>5}  repair-only overhead  median {:.2}  p90 {:.2}  max {:.2}",
            v[15], v[27], v[29]
        );
    }
}

/// Frames needed from the start of the carousel at a given loss, fountain against a plain
/// carousel that cycles manifest + chunks: `... survey_vs_carousel -- --ignored --nocapture`.
#[test]
#[ignore]
fn survey_vs_carousel() {
    for k in [100u32, 1000, 3000] {
        let (_, f) = fountain(k as usize * 200, 200);
        let kk = f.source_count() as u64;
        for loss in [0.1, 0.3, 0.5] {
            let mut fount = Vec::new();
            let mut plain = Vec::new();
            for seed in 1..=10u64 {
                fount.push(watch(&f, 0, loss, seed * 0x9E37).1 as f64 / kk as f64);
                // Plain carousel: the same drop pattern, chunks only, cycling.
                let mut coin = Coin((seed * 0x9E37) | 1);
                let mut seen = vec![false; kk as usize];
                let (mut left, mut n) = (kk, 0u64);
                while left > 0 {
                    let i = (n % (kk + 1)) as usize; // slot 0 of each loop is the manifest
                    n += 1;
                    if coin.flip(loss) || i == 0 {
                        continue;
                    }
                    if !seen[i - 1] {
                        seen[i - 1] = true;
                        left -= 1;
                    }
                }
                plain.push(n as f64 / kk as f64);
            }
            let mean = |v: &Vec<f64>| v.iter().sum::<f64>() / v.len() as f64;
            println!(
                "K={kk:>5} loss {:>2.0}%   frames per chunk: fountain {:.2}   plain carousel {:.2}",
                loss * 100.0,
                mean(&fount),
                mean(&plain)
            );
        }
    }
}
