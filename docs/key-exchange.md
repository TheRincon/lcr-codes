# Offline device enrollment and sync-key transfer

**Status: draft proposal.** Schema IDs and field labels are placeholders pending the §11
registry.

Two operations, both fully offline, both carried entirely over LCR Code symbols shown and
scanned between two devices:

1. **Enrollment** — an already-trusted device vouches for a new one, so the new device can be
   admitted (and can then create its *own* passkey with a relying party).
2. **Sync-key transfer** — one trusted device hands another a short-lived secret for offline
   work or access.

---

## 1. The constraint that shapes everything

### 1.1 Passkeys cannot be exported, and this spec does not try

A passkey is a FIDO2/WebAuthn credential whose private key lives in an authenticator — a
platform keychain or a hardware key. WebAuthn offers **no API to export that private key**,
and platform authenticators do not expose it. That is the security model working as
intended, not a gap to route around.

So this spec never moves a passkey private key. The two sanctioned routes for what people
usually mean by "moving a passkey" already exist and should be used:

| goal | route |
|---|---|
| use a passkey on a nearby device | CTAP 2.2 *hybrid* transport (QR + BLE + relay) |
| move passkeys between credential providers | FIDO Alliance Credential Exchange (CXP/CXF) |
| **add a device to an account, fully offline** | **this spec, §4** — then the new device registers its own passkey |
| **carry a short-lived app secret, fully offline** | **this spec, §5** |

The gap LCR fills is **fully offline, camera-only**. CTAP hybrid needs a tunnel server and BLE;
this needs two screens and two cameras.

### 1.2 "Signed" is not "safe" — §10.8 already says why

§10.8 is normative and blunt: *no confidentiality* — anyone in camera range can capture the
symbol; *screenshots are trivial* — a static symbol is not a bearer credential for anything of
value. A §10 signature proves **origin and integrity**. It does nothing to stop the wrong
person from *reading* a secret. A signed secret is a readable secret.

This spec therefore rests on two further mechanisms:

- **Encryption to a key the observer never sees** (§5) — for anything secret.
- **Human confirmation of a short code** (§4) — for establishing trust where none exists.

---

## 2. Threat model

| attacker | capability | defended by |
|---|---|---|
| shoulder-surfer / CCTV | photographs any symbol shown | HPKE to an ephemeral key (§5); no secret in §4 at all |
| screenshot / screen recorder | copies any displayed symbol | as above, plus single-use keys and short `exp` |
| replayer | re-shows an old symbol later | nonce binding, single-use ephemeral keys, `exp` (§6) |
| **active interposer** | shows device A a symbol of their own instead of device B's | mutual signatures (§5) or commitment-based SAS (§4) |
| lost device | holds an old sync key | the key's own short lifetime; §10.6 key rotation |

**Out of scope:** a device already compromised (malware reads the decrypted secret), and a
user who confirms a short code without comparing it. The protocol makes the second as hard to
do by accident as it can; it cannot make it impossible.

---

## 3. Structure — two tiers, split by whether trust exists yet

The central design point is that **the two hard problems never occur in the same exchange**:

| | trust between devices | secret on the wire | symbols | human step |
|---|---|---|---|---|
| **Tier 1 — Enrollment** | none yet — being *established* | **none** | 4 | compare a 6-digit code |
| **Tier 0 — Sync key** | already established | **yes**, encrypted | 2 | none |

Enrollment is where an interposer is most dangerous, and it carries nothing secret — only
public keys and a signature. The sync key is secret, and it only ever moves between devices
that already hold each other's identity keys. A deployment that needs both runs Tier 1 once,
then Tier 0 as often as it likes.

Every *signed* message in either tier is an ordinary **§10 signed packet** (`ctype = 0x4`,
COSE_Sign1 + CWT, with `lcr.schema` naming the message and `lcr.body` carrying it). That reuses
`lcr-verify` unchanged — including §10.5's KeyResolver and §10.6's mandatory `exp` — rather
than inventing a second signing path. Unsigned messages are plain CBOR (`ctype = 0x3`).

---

## 4. Tier 1 — Enrollment

### 4.1 Why it needs four symbols, not two

The obvious two-symbol version — new device shows its public key, old device signs it — is
**broken by an interposer**, and the obvious fix is too. Showing both devices a short code
derived from the exchanged keys does not help if either side can choose its key *after*
seeing the other's: the attacker grinds candidate keys until the 6-digit code collides,
which takes about a million tries and a fraction of a second.

The standard defence is a **commitment**: one side binds itself to its values before it has
seen the other's, so neither side can grind. It is the construction behind Bluetooth Secure
Simple Pairing's *numeric comparison* and ZRTP's short authentication string, and it costs
exactly one extra message. With it, an interposer's chance per attempt is `10⁻⁶` for a
6-digit code, and each attempt burns the whole exchange.

### 4.2 Flow

`N` is the new device. `O` is the old, already-trusted device.

```
  N (new)                                            O (old, trusted)

  1. commit     c = H(cbor(N_key) ‖ rN)   ────────▶  scans
  2.   scans                              ◀────────  reveal   O_key, rO
  3. open       N_key, rN                 ────────▶  scans; checks c
                     ┌───────────────────────────────────────┐
                     │  both display SAS, user compares them │
                     └───────────────────────────────────────┘
  4.   scans                              ◀────────  authorization  (signed by O)
```

1. **Commit** (`N` → `O`, unsigned). `N` shows `c = SHA-256("LCR-ENROLL-v1" ‖ cbor(N_key) ‖ rN)`,
   binding it to its identity key and a fresh 16-byte nonce `rN` before seeing anything of
   `O`'s.
2. **Reveal** (`O` → `N`, unsigned). `O` shows its identity public key `O_key` and a fresh nonce
   `rO`. It has not yet seen `N_key` or `rN` — only their hash.
3. **Open** (`N` → `O`, unsigned). `N` shows `N_key` and `rN`. `O` recomputes `c` and **aborts on
   mismatch**.
4. **Short authentication string.** Both devices compute and display

   ```
   SAS = u32(SHA-256("LCR-SAS-v1" ‖ c ‖ cbor(O_key) ‖ rO ‖ cbor(N_key) ‖ rN)[0..4]) mod 10⁶
   ```

   as six digits. The user confirms they match **on `O`**. Until then `O` **MUST NOT** proceed.
5. **Authorization** (`O` → `N`, **signed** by `O`). A §10 packet whose body names `N_key` as
   authorized, bound to the transcript:

   ```
   T = SHA-256("LCR-ENROLL-TRANSCRIPT-v1" ‖ c ‖ cbor(O_key) ‖ rO ‖ cbor(N_key) ‖ rN)
   ```

### 4.3 What `N` does with it

`N` verifies the authorization with §10 as usual — its KeyResolver returns `O_key`.

That looks like it breaks §10.5's one non-negotiable rule, *verifiers MUST NOT accept a public
key transported in the symbol*, and the distinction is worth stating exactly. §10.5 forbids
using a key from a symbol to verify **that same symbol**, which makes the check circular — an
attacker supplies both. Here `O_key` arrived in message 2 and was then **authenticated out of
band** by the user comparing the SAS in step 4. `N` pins it only after that comparison, so it
is trust-on-first-use backed by a human check, not a circular one. A `N` that pins `O_key`
before the SAS is confirmed is non-conformant.

The authorization is then presented to whatever admits devices: an application's own backend,
a local offline verifier, or — where a relying party supports it — as the basis for letting
`N` sign in and register **its own** passkey through the ordinary WebAuthn ceremony. How a
relying party consumes it is outside this spec; a standard third-party website will not
understand it without adopting it.

---

## 5. Tier 0 — Sync-key transfer

For devices that **already hold each other's identity keys**, typically after Tier 1.

### 5.1 Flow

`R` receives the secret. `S` sends it.

```
  R (receiver)                                       S (sender)

  1. request   signed by R:  R_eph, nonce  ────────▶  verifies R's signature
  2.   scans; verifies S; decrypts         ◀────────  response  signed by S:
                                                       HPKE-seal(R_eph, secret)
```

1. **Request** (`R` → `S`, signed by `R`). A fresh ephemeral HPKE public key `R_eph`, a 16-byte
   nonce, and the suite `R` supports. `S` **MUST** verify `R`'s signature before doing
   anything else — that is what stops an interposer substituting their own `R_eph`, which the
   response's encryption would otherwise faithfully honour.
2. **Response** (`S` → `R`, signed by `S`). The secret, sealed with HPKE to `R_eph`:

   ```
   info = "LCR-SYNC-v1" ‖ nonce
   aad  = SHA-256(request packet bytes)
   ```

   `info` and `aad` bind the ciphertext to this one request; a response cannot be replayed
   against a different one.

### 5.2 Why this survives a camera

An observer who photographs both symbols holds `R_eph` (public), the HPKE encapsulated key
(public), and the ciphertext. Opening it needs `R_eph`'s **private** key, which never left
`R` and is discarded after this exchange. Nothing an observer can see decrypts it — which is
exactly the property §10.8 says a signature alone cannot give.

### 5.3 The secret itself

The payload is an **application-controlled** secret: a session key, an offline access token,
an app's own signing key. Never a WebAuthn private key (§1.1). It carries its own lifetime,
independent of the packet's `exp`, because a sync key's useful life and its transfer window
are different things — the transfer window should be minutes; the key's life is a
deployment decision, and §10.6's argument applies: **it is the entire blast radius**.

---

## 6. Freshness and replay

| mechanism | applies to | bound |
|---|---|---|
| `exp` on every signed packet | both tiers | **REQUIRED** (§10.6). ≤ 300 s for exchange messages |
| nonce echoed in response, bound in `info`/`aad`/`T` | both tiers | one response per request |
| ephemeral keys used once, then destroyed | Tier 0 `R_eph`; Tier 1 `rN`, `rO` | a replayed response opens nothing |
| sync secret's own lifetime | Tier 0 payload | deployment decision; keep it short |

A verifier without a trustworthy clock fails closed, exactly as §10.6 requires.

---

## 7. Algorithms

Mirroring §10.3's rule: **choose by where the key lives, not by elegance**, and receivers
**MUST** implement every suite so the ecosystem does not fracture.

| purpose | software keys | hardware keystores (Secure Enclave, StrongBox) |
|---|---|---|
| signature (§10.3, unchanged) | Ed25519 | ES256 |
| HPKE (RFC 9180) KEM | DHKEM(X25519, HKDF-SHA256) | DHKEM(P-256, HKDF-SHA256) |
| HPKE KDF | HKDF-SHA256 | HKDF-SHA256 |
| HPKE AEAD | ChaCha20-Poly1305 | AES-128-GCM |
| commitment, SAS, transcript | SHA-256 with the domain labels above | same |

HPKE rather than hand-assembled ECDH + HKDF + AEAD: it is a standardised, analysed
composition of exactly those parts, and the COSE mapping (`draft-ietf-cose-hpke`) is the
natural wire form once it settles.

---

## 8. Wire format

CDDL (RFC 8610). Integer keys throughout, per §11's argument that index keys are where the
capacity lives. Every body starts with `0 => version`; a receiver **MUST** reject a version it
does not implement rather than guessing — the same rule CFT §4 and this project apply
everywhere.

```cddl
; ---- shared ----
version    = 1
nonce      = bstr .size 16
cose-key   = { * (int / tstr) => any }   ; RFC 9052 COSE_Key, public parameters only
suite      = &( x25519-chacha: 1, p256-aesgcm: 2 )

; ---- Tier 1: enrollment -------------------------------------------------------
; lcr.schema values are placeholders pending the §11 registry.

enroll-commit = {            ; ctype 0x3, unsigned, lcr.schema 0xF101
  0 => version,
  1 => bstr .size 32,        ; c
}

enroll-reveal = {            ; ctype 0x3, unsigned, lcr.schema 0xF102
  0 => version,
  2 => cose-key,             ; O_key
  3 => nonce,                ; rO
}

enroll-open = {              ; ctype 0x3, unsigned, lcr.schema 0xF103
  0 => version,
  4 => cose-key,             ; N_key
  5 => nonce,                ; rN
}

enroll-authorization = {     ; lcr.body of a §10 packet signed by O, lcr.schema 0xF104
  0 => version,
  4 => cose-key,             ; N_key — the key being admitted
  6 => bstr .size 32,        ; T, the transcript hash
  ? 7 => tstr,               ; scope: what N is admitted to
}

; ---- Tier 0: sync key --------------------------------------------------------

sync-request = {             ; lcr.body of a §10 packet signed by R, lcr.schema 0xF111
  0 => version,
  8 => cose-key,             ; R_eph — ephemeral HPKE public key
  3 => nonce,
  9 => [+ suite],            ; suites R accepts, preferred first
}

sync-response = {            ; lcr.body of a §10 packet signed by S, lcr.schema 0xF112
  0 => version,
  3 => nonce,                ; echoes the request's
  10 => suite,               ; the one S chose
  11 => bstr,                ; HPKE encapsulated key
  12 => bstr,                ; HPKE ciphertext of sync-secret
}

sync-secret = {              ; the HPKE plaintext — never on the wire in the clear
  0 => version,
  13 => tstr,                ; purpose, e.g. "offline-access"
  14 => bstr,                ; the key material
  15 => uint,                ; the key's own expiry, seconds since epoch
  ? 16 => any,               ; application metadata
}
```

The `0xF1xx` values sit in §11's private-use range so they can be implemented today without a
collision risk that matters; they move into the registered range when §11 has an allocation
process.

---

## 9. Size

Measured, not estimated: each message is built exactly as §8 specifies — Ed25519 signatures,
OKP COSE_Keys, the §9.1 frame header and CRC included — and matched against the capacity
table at `C3`, ECC 1, the setting that decodes on the field hardware.
`cargo run -p lcr-encode --example keyx_sizes` reproduces this table.

| message | measured | smallest symbol |
|---|---:|---|
| enroll-commit | 42 B | v1 (108 B) |
| enroll-reveal / enroll-open | 66 B | v1 (108 B) |
| enroll-authorization (signed) | 202 B | v3 (202 B) — **exactly full** |
| sync-request (signed) | 173 B | v3 (202 B) |
| sync-response, 200 B secret | 417 B | v7 (480 B) |
| sync-response, 1,400 B secret | 1,619 B | v17 (1,688 B) |

Two things follow.

**The 200–1,400-byte range is well inside a single ordinary symbol.** The largest case needs
v17 — nowhere near the extended versions. Density is not the problem here; the envelope is,
and §10.4's point that *the signature dominates* shows up plainly: a 42-byte unsigned commit
becomes a 202-byte signed authorization.

**The signed authorization has no slack at v3.** 202 bytes in a 202-byte symbol, with a
14-character scope string. A longer scope, or ES256's DER-free but differently-shaped COSE
header, pushes it to v4. Encoders should let version selection run rather than pinning v3.

---

## 10. Implementation notes

**A separate crate, and it has to be.** Both tiers generate fresh ephemeral keys and nonces on
*both* devices, which needs a random number generator. §14.2 makes `getrandom` appearing in
the verify path a build failure, specifically so a signing path cannot leak into a scanner.
This protocol cannot live in `lcr-verify`; a crate such as `lcr-pair` sits above it, depends on
`lcr-verify` for the signed messages, and brings the RNG and HPKE with it.

**The crate is a pair of state machines, not a UI.** It exposes the transitions — commit,
reveal, open, confirm, authorize; request, respond, open — and the SAS as a value. Showing the
six digits and asking the user to compare them is the application's job, and the one step the
protocol's security rests on. The state machine refuses to produce an authorization until the
application reports the comparison succeeded.

**Things a conforming implementation MUST NOT do:**

- place a WebAuthn private key in any message;
- sign a Tier 1 authorization, or pin `O_key`, before the SAS comparison succeeds;
- encrypt a Tier 0 response before verifying the request's signature;
- reuse an ephemeral key or nonce across exchanges;
- accept an unknown `version`;
- present an unverified or undecryptable message as success (§10.8's fail-closed rule).

---

## 11. Open questions

1. **Relying-party integration.** §4.3's authorization is only as useful as whatever consumes
   it. For the user's own systems that is straightforward; for third-party websites it needs
   their adoption, and CTAP hybrid or CXP may be the better answer there.
2. **SAS presentation.** Six digits matches Bluetooth's numeric comparison. A word list is
   easier to compare aloud and harder to fumble, at the cost of a localisation problem.
3. **One-way variant.** Where `R` cannot display a symbol (a headless device), Tier 0 has no
   channel back. A pre-provisioned `R_eph` is possible but loses the single-use property, and
   should be specified separately rather than weakening this one.
4. **Revocation.** None, per §10.6 — the answer is short lifetimes and key rotation. Worth
   stating in user-facing docs for any deployment that issues long-lived sync keys.
