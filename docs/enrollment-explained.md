# Enrollment, explained

A plain-language companion to `docs/key-exchange.md` §4. No new rules here — just why the
flow looks the way it does.

---

## The goal, in one sentence

Your **old phone** (already trusted) should vouch for your **new phone**, so the new phone can
be admitted — without anyone being able to sneak *their* device in instead.

Nothing secret changes hands. The new phone just needs a note from the old one that says
*"this key is mine, and it's OK."*

---

## Attempt 1: just show the key and sign it

```
new phone  ── "here's my key" ──▶  old phone signs it
```

**Broken.** An attacker holds their own screen in front of the old phone's camera. The old
phone can't tell which screen is yours, so it signs the attacker's key. Now the attacker's
device is admitted.

The fix is obvious: make both phones show a short code, and have you check they match.

## Attempt 2: swap keys, then compare a 6-digit code

```
new phone ◀── keys ──▶ old phone      both show  ###-###      you compare
```

**Still broken, and this is the subtle part.** The attacker sits in the middle, showing each
phone a different key of its own. Each screen's code depends on the keys that phone saw. The
attacker picks the key it shows the old phone, *looks at the code that appears there*, then
tries key after key for the new phone until that screen's code comes out the same.

Six digits is a million possibilities. A computer tries a million keys in about a second.
The demo at the bottom does exactly this:

```
NAIVE: screens both show 430979 — attacker won after 524,610 tries in 1.6s
```

The codes match, you tap OK, and the attacker is in. The code comparison didn't help because
**the attacker got to choose after seeing what it had to match.**

## Attempt 3 (the real one): seal it first

Take that choice away. Before anything else, the new phone **seals** its answer — puts its
key and a random number into an envelope nobody can see into — and hands over the sealed
envelope.

```
  NEW PHONE                                     OLD PHONE

  1. seal     "here's a sealed envelope"  ───▶  holds it, can't read it
  2.          "my key, and my random number" ◀─── reveal
  3. open     "here's what was inside"    ───▶  checks it matches the seal

              both screens now show a 6-digit code — you compare them

  4.          "signed: your key is OK"    ◀───  signs, ONLY after you confirm
```

The envelope is a hash — `c = SHA-256(key ‖ random)` — which you can check against later but
can't work backwards from.

**Why the attacker is now stuck.** To make your two screens match, the attacker has to know
what code the *new* phone will show. That code depends on what's inside the new phone's
envelope. And the attacker has to commit to its own values *before* that envelope is opened.
It is trying to hit a target it can't see.

It can still grind — but only against a *guess* of what's in the envelope. The demo:

```
COMMITTED: attacker's grind 'succeeded' against its guess after 602,067 tries
           O shows 848848, N shows 014450 -> MISMATCH — user sees the attack
```

A million tries buys it nothing, because it was matching the wrong thing. Its real chance is a
blind one-in-a-million guess per attempt — and every miss shows you two different codes.

This isn't invented here: it is how Bluetooth's *numeric comparison* pairing works, for the
same reason.

---

## What you actually do

1. On the **new** phone, tap *Add this device*. It shows a symbol.
2. Point the **old** phone at it. The old phone shows a symbol.
3. Point the **new** phone at that. The new phone shows another symbol.
4. Point the **old** phone at it. **Both now show six digits.**
5. **Look at both. If they match, tap Confirm on the old phone.** If they don't, stop — someone
   is interfering.
6. The old phone shows one last symbol. Point the new phone at it. Done.

Four scans and one comparison. Step 5 is the only one that needs you to pay attention, and it
is the whole reason the rest is safe.

---

## What comes out of it

A **signed note** from the old phone: *"key N belongs here, valid until `<time>`."* It's an
ordinary §10 signed packet, so the existing verifier checks it.

**Nothing secret ever appeared on either screen** — only public keys, random numbers and a
signature. Someone photographing every symbol learns nothing they could use, which is why
enrollment doesn't need encryption at all.

## "Doesn't the new phone just trust a key it scanned?"

That rule — *never trust a key that arrived inside the symbol* (§10.5) — is there to stop a
symbol vouching for **itself**: an attacker supplies both the signature and the key that
checks it, and the check is meaningless.

Here the old phone's key arrived in step 2, and then **you** checked the six digits in step 5.
The new phone only starts trusting it *after* that. You are the second channel. That's the
difference between a circular check and a real one.

---

## And where does hybrid fit?

Your clarification — you meant CTAP's **hybrid** flow, not a sync key — makes the second half
simpler, not harder.

Hybrid is *"use the passkey on my phone to sign in on this laptop."* Offline, over symbols, that
is just:

```
  laptop  ── "sign this challenge: <random>, for <app>" ──▶  phone
  laptop  ◀── "signed: <signature>" ──────────────────────   phone asks you first
```

**Nothing secret moves** — a signature over a fresh random challenge isn't a secret, and it is
useless for any other challenge. So it needs no encryption, unlike the sync-key tier in
`key-exchange.md` §5, which was built for a different problem.

Two things make it safe: the laptop only accepts a signature from a key it learned during
**enrollment** (which is what enrollment is *for*), and the phone **shows you what you're
signing into** before it signs — your defence against someone relaying a challenge from
somewhere else.

One real limit: a web page can't add "LCR symbols" as a way for the browser to reach an
authenticator — browsers only use the transports they build in (USB, NFC, Bluetooth, and
their own hybrid). So this works where **you** check the signature — your own app or backend.
For ordinary third-party websites, the platform's own hybrid flow is the route.

---

## Run the demo yourself

```python
import hashlib, os, time

H = lambda *p: hashlib.sha256(b"".join(p)).digest()
def code(*parts):                       # the six digits each screen shows
    return int.from_bytes(H(b"LCR-SAS-v1", *parts)[:4], "big") % 1_000_000

# ============ 1. Naive version: swap keys, then show a code ============
N_key, O_key = os.urandom(32), os.urandom(32)
A1 = os.urandom(32)                      # attacker's key, shown to O
on_O = code(A1, O_key)                   # O's screen, fixed now
t0, tries = time.time(), 0
while True:                              # attacker grinds the key it shows N
    tries += 1
    A2 = os.urandom(32)
    if code(N_key, A2) == on_O:
        break
print(f"NAIVE: screens both show {on_O:06d} — attacker won after {tries:,} tries "
      f"in {time.time()-t0:.1f}s")

# ============ 2. Real protocol: commit, reveal, open ============
N_key, rN = os.urandom(32), os.urandom(16)
cN = H(b"LCR-ENROLL-v1", N_key, rN)       # the sealed envelope

A1, rA1 = os.urandom(32), os.urandom(16) # attacker plays "new phone" to O
cA = H(b"LCR-ENROLL-v1", A1, rA1)
O_key, rO = os.urandom(32), os.urandom(16)
on_O = code(cA, O_key, rO, A1, rA1)      # O's code is now fixed

# Attacker plays "old phone" to N and must reveal BEFORE N opens its envelope, so it can
# only grind against a guess of what's inside.
guess_key, guess_r = os.urandom(32), os.urandom(16)
tries = 0
while True:
    tries += 1
    A2, rA2 = os.urandom(32), os.urandom(16)
    if code(cN, A2, rA2, guess_key, guess_r) == on_O:
        break
print(f"COMMITTED: attacker's grind 'succeeded' against its guess after {tries:,} tries")

on_N = code(cN, A2, rA2, N_key, rN)      # N opens the envelope: the real code
print(f"           O shows {on_O:06d}, N shows {on_N:06d} -> "
      f"{'MATCH (attack worked)' if on_N == on_O else 'MISMATCH — user sees the attack'}")
```

Run it a few times. The naive version always falls in a second or two. The committed one
mismatches — about one run in a million it will match by pure chance, which is the
one-in-a-million the six digits are there to give you.
