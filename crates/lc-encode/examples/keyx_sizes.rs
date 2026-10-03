//! Measure docs/key-exchange.md §9: build each message as specified and find the smallest
//! symbol that carries it. Scaffolding for the spec, not shipped code.
use lc_core::cbor::Value;
use lc_core::{EccLevel, Profile, Version};
use lc_encode::sign::{SignOptions, SigningKey, sign};

fn uint(k: i64, v: u64) -> (Value, Value) {
    (Value::from_i64(k), Value::Uint(v))
}
fn bytes(k: i64, n: usize) -> (Value, Value) {
    (Value::from_i64(k), Value::Bytes(vec![0xA5; n]))
}
fn okp(crv: i64) -> Value {
    // RFC 9052 COSE_Key, OKP: kty 1, crv, x — public parameters only.
    Value::Map(vec![
        (Value::from_i64(1), Value::from_i64(1)),
        (Value::from_i64(-1), Value::from_i64(crv)),
        (Value::from_i64(-2), Value::Bytes(vec![0x42; 32])),
    ])
}
fn smallest(len: usize) -> String {
    for v in 1..=Version::MAX.get() {
        let cap = lc_encode::capacity_for_symbol(
            Version::new(v).unwrap(),
            Profile::C3,
            EccLevel::new(1, 1).unwrap(),
            true,
        );
        if cap.body_bytes >= len {
            return format!("v{v} ({} B cap)", cap.body_bytes);
        }
    }
    "none".into()
}
fn main() {
    let key = SigningKey::from_ed25519_bytes(&[7u8; 32]);
    let opts = SignOptions::expiring_in(1_800_000_000, 300);
    let signed = |body: Value| sign(&key, &body.to_vec(), &opts).unwrap().len();

    let commit = Value::Map(vec![uint(0, 1), bytes(1, 32)]).to_vec().len();
    let reveal = Value::Map(vec![uint(0, 1), (Value::from_i64(2), okp(6)), bytes(3, 16)])
        .to_vec()
        .len();
    let auth = signed(Value::Map(vec![
        uint(0, 1),
        (Value::from_i64(4), okp(6)),
        bytes(6, 32),
        (Value::from_i64(7), Value::Text("offline-access".into())),
    ]));
    let req = signed(Value::Map(vec![
        uint(0, 1),
        (Value::from_i64(8), okp(4)),
        bytes(3, 16),
        (
            Value::from_i64(9),
            Value::Array(vec![Value::Uint(1), Value::Uint(2)]),
        ),
    ]));
    let resp = |secret: usize| {
        // sync-secret plaintext, then + 16 B AEAD tag, then the sealed response body.
        let pt = Value::Map(vec![
            uint(0, 1),
            (Value::from_i64(13), Value::Text("offline-access".into())),
            bytes(14, secret),
            uint(15, 1_800_086_400),
        ])
        .to_vec()
        .len();
        signed(Value::Map(vec![
            uint(0, 1),
            bytes(3, 16),
            uint(10, 1),
            bytes(11, 32),
            bytes(12, pt + 16),
        ]))
    };
    println!("| message | measured | smallest C3 ECC 1 symbol |");
    println!("|---|---:|---|");
    for (n, b) in [
        ("enroll-commit", commit),
        ("enroll-reveal / open", reveal),
        ("enroll-authorization (signed)", auth),
        ("sync-request (signed)", req),
        ("sync-response, 200 B secret", resp(200)),
        ("sync-response, 1,400 B secret", resp(1400)),
    ] {
        // + the §9.1 frame: header and CRC.
        let framed = b + if b <= 255 { 4 } else { 7 };
        println!("| {n} | {framed} B | {} |", smallest(framed));
    }
}
