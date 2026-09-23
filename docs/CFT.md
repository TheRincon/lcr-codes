# CFT — CBOR File Transfer Format

A small, transport-independent file-transfer format using:

* **CBOR** for the envelope and metadata
* **Zstandard (Zstd)** for compression
* **QR codes, BLE, serial, network, etc.** as transports
* **Hashes** for end-to-end integrity
* **Transport-level error correction** where available

The design goal is to keep the format simple, compact, and suitable for constrained transports such as QR codes.

---

## 1. Architecture

The overall pipeline is:

```text
Original file
     │
     ▼
  Zstandard
     │
     ▼
Compressed bytes
     │
     ▼
Split into chunks
     │
     ▼
CBOR envelope
     │
     ▼
Transport
(QR / BLE / serial / network)
```

On the receiving side:

```text
Transport
     │
     ▼
Reassemble chunks
     │
     ▼
Extract Zstd stream
     │
     ▼
Zstandard decompress
     │
     ▼
Verify original-file hash
     │
     ▼
Original file
```

The important architectural principle is:

> CFT does not depend on the transport.

QR is one possible transport, but the same CFT object can be transferred over BLE, serial, TCP, HTTP, NFC, or another mechanism.

---

# 2. File Transfer Structure

A transfer consists of:

1. A **manifest**
2. One or more **data chunks**

Conceptually:

```text
Transfer
├── Manifest
│   ├── Protocol/version
│   ├── Filename
│   ├── Original size
│   ├── Compressed size
│   ├── Compression
│   ├── Hash algorithm
│   ├── Original-file hash
│   └── Chunk size
│
└── Data chunks
    ├── Chunk 0
    ├── Chunk 1
    ├── Chunk 2
    └── ...
```

---

# 3. Manifest

The manifest is a CBOR map.

A conceptual representation:

```text
{
  0: "CFT1",
  1: "example.pdf",
  2: 1234567,
  3: 456789,
  4: "sha256",
  5: h'ABCD...',
  6: 4096,
  7: "zstd"
}
```

Suggested field meanings:

| Key | Meaning            | Type             |
| --: | ------------------ | ---------------- |
| `0` | Protocol/version   | text             |
| `1` | Filename           | text             |
| `2` | Original file size | unsigned integer |
| `3` | Compressed size    | unsigned integer |
| `4` | Hash algorithm     | text             |
| `5` | Original-file hash | byte string      |
| `6` | Chunk size         | unsigned integer |
| `7` | Compression format | text             |

Integer keys are deliberately used to keep the CBOR representation compact.

---

# 4. Protocol Version

The manifest contains:

```text
0: "CFT1"
```

This identifies the protocol and version.

Future versions could use:

```text
"CFT2"
"CFT3"
```

etc.

Implementations should reject versions they do not understand rather than silently interpreting them as another version.

---

# 5. Compression

The initial format supports:

```text
7: "zstd"
```

The data being chunked is the **compressed Zstandard byte stream**.

The pipeline is therefore:

```text
Original file
      │
      ▼
   Zstandard
      │
      ▼
Compressed byte stream
      │
      ├── chunk 0
      ├── chunk 1
      ├── chunk 2
      └── ...
```

The chunks should not be independently decompressed unless the protocol is explicitly extended to support independently compressed chunks.

This design keeps the compressed representation as a single Zstd stream.

---

# 6. Data Chunks

Each chunk is a CBOR map.

Conceptually:

```text
{
  0: transfer_id,
  1: chunk_number,
  2: total_chunks,
  3: h'...binary zstd data...'
}
```

Suggested fields:

| Key | Meaning                | Type                   |
| --: | ---------------------- | ---------------------- |
| `0` | Transfer ID            | integer or byte string |
| `1` | Chunk number           | unsigned integer       |
| `2` | Total number of chunks | unsigned integer       |
| `3` | Chunk payload          | byte string            |

The payload is the raw Zstd byte data.

CBOR's byte-string type is important here because the compressed data should **not** be converted to base64.

---

# 7. Example Transfer

Suppose we have:

```text
example.pdf
```

with an original size of:

```text
1,234,567 bytes
```

After Zstandard compression:

```text
456,789 bytes
```

With a chunk size of:

```text
4096 bytes
```

the compressed stream is divided into approximately:

```text
112 chunks
```

The manifest might conceptually contain:

```text
{
  0: "CFT1",
  1: "example.pdf",
  2: 1234567,
  3: 456789,
  4: "sha256",
  5: h'...32 byte SHA-256 digest...',
  6: 4096,
  7: "zstd"
}
```

A data chunk might contain:

```text
{
  0: 42,
  1: 17,
  2: 112,
  3: h'...4096 bytes...'
}
```

Here:

```text
42   = transfer ID
17   = chunk number
112  = total chunks
3    = Zstd payload
```

---

# 8. Transfer ID

The transfer ID identifies all chunks belonging to the same transfer.

For example:

```text
0: 42
```

could identify transfer 42.

For a more robust implementation, a random byte string is preferable:

```text
0: h'8F13A92C7E...'
```

This avoids collisions between simultaneous or repeated transfers.

---

# 9. Chunk Ordering

Chunks are numbered starting from zero:

```text
0
1
2
3
...
N-1
```

The receiver can therefore accept chunks in any order.

For example:

```text
Chunk 0
Chunk 7
Chunk 3
Chunk 1
Chunk 6
Chunk 2
...
```

The receiver stores them until all chunks have arrived and then reconstructs the compressed stream in numerical order.

---

# 10. Missing Chunks

The protocol should not assume that chunks arrive reliably.

The receiver maintains a set such as:

```text
Received:

0 ✓
1 ✓
2 ✗
3 ✓
4 ✓
5 ✗
6 ✓
```

The missing chunks can then be requested again if the transport supports retransmission.

For a QR-code transport, the receiver could display or communicate:

```text
Missing:
2, 5
```

and the sender could rescan only those QR codes.

---

# 11. Integrity

The manifest contains a hash of the **original, uncompressed file**.

For example:

```text
4: "sha256"
5: h'...'
```

The important sequence is:

```text
Original file
     │
     ├──────────── SHA-256 ────────────► Manifest
     │
     ▼
   Zstd
     │
     ▼
Compressed stream
     │
     ▼
   Chunks
```

After reconstruction:

```text
Chunks
  │
  ▼
Reassembled Zstd stream
  │
  ▼
Zstd decompression
  │
  ▼
Original file
  │
  ▼
SHA-256
  │
  ▼
Compare with manifest
```

Only when the hashes match should the reconstructed file be considered valid.

This provides an **end-to-end integrity check** independent of the transport.

---

# 12. Why Hash the Original File?

Hashing the original file means that the final verification covers the entire pipeline:

```text
Original
   ↓
Compression
   ↓
Chunking
   ↓
Transport
   ↓
Reassembly
   ↓
Decompression
   ↓
Original
```

A successful hash comparison means the final file is identical to the original input.

A chunk-level checksum alone would not provide the same end-to-end guarantee.

---

# 13. QR Code Transport

QR can be treated as simply another transport.

The architecture becomes:

```text
                     CFT
                      │
                 CBOR chunks
                      │
                      ▼
              ┌───────────────┐
              │ QR encoding   │
              └───────────────┘
                      │
                      ▼
             QR Reed-Solomon ECC
                      │
                      ▼
                 QR modules
```

The QR code's own Reed-Solomon error correction operates **below CFT**.

CFT therefore does not need to know that the transport is QR.

---

# 14. QR Transfer

A QR-oriented implementation might use:

```text
┌─────────────────────────────┐
│ CFT / CBOR header           │
├─────────────────────────────┤
│ Transfer ID                 │
│ Chunk number                │
│ Total chunks                │
├─────────────────────────────┤
│ Zstd payload                │
├─────────────────────────────┤
│ Optional chunk integrity    │
└─────────────────────────────┘
              │
              ▼
       QR Reed-Solomon ECC
              │
              ▼
          QR modules
```

The QR specification's own Reed-Solomon ECC provides protection against damaged or unreadable QR modules.

CFT's end-to-end hash provides protection against incorrect or incomplete reconstruction.

These are different layers of protection.

---

# 15. Two Levels of Error Protection

There are therefore two distinct mechanisms:

```text
QR layer
─────────
Protects the QR representation itself.

        ↓

CFT layer
─────────
Verifies that the reconstructed file is correct.
```

For example:

```text
Damaged QR module
       │
       ▼
QR Reed-Solomon
       │
       ▼
Recovered chunk
       │
       ▼
CFT reconstruction
       │
       ▼
Zstd decompression
       │
       ▼
SHA-256 verification
```

---

# 16. Optional Chunk Hash

A future CFT version could optionally add a per-chunk hash:

```text
{
  0: transfer_id,
  1: chunk_number,
  2: total_chunks,
  3: h'...payload...',
  4: h'...chunk hash...'
}
```

This allows a receiver to detect a corrupt chunk immediately rather than waiting until the entire file has been reconstructed and decompressed.

However, the original-file hash remains the authoritative end-to-end integrity check.

---

# 17. Transport Independence

The same CFT transfer can be carried over different transports:

```text
                    CFT
                     │
          ┌──────────┼──────────┐
          │          │          │
          ▼          ▼          ▼
         QR         BLE       Network
          │          │          │
          ▼          ▼          ▼
       Transport-specific framing
```

CFT should therefore avoid including QR-specific concepts in its core format.

For example, CFT should not contain fields such as:

```text
qr_version
qr_mask
qr_error_correction_level
```

Those belong to the QR transport layer.

---

# 18. Recommended Layering

A clean implementation can be divided into four layers:

```text
┌─────────────────────────────┐
│ Application                 │
│ filename, user interaction  │
└──────────────┬──────────────┘
               │
┌──────────────▼──────────────┐
│ CFT                         │
│ CBOR, chunks, hashes        │
└──────────────┬──────────────┘
               │
┌──────────────▼──────────────┐
│ Compression                 │
│ Zstandard                   │
└──────────────┬──────────────┘
               │
┌──────────────▼──────────────┐
│ Transport                   │
│ QR / BLE / serial / network │
└─────────────────────────────┘
```

---

# 19. Complete Example

The complete lifecycle is:

```text
                 example.pdf
                      │
                      ▼
                   Zstd
                      │
                      ▼
              456,789 bytes
                      │
                      ▼
                 chunking
                      │
        ┌─────────────┼─────────────┐
        ▼             ▼             ▼
     Chunk 0       Chunk 1       Chunk 2
        │             │             │
        ▼             ▼             ▼
       CBOR          CBOR          CBOR
        │             │             │
        └─────────────┼─────────────┘
                      │
                      ▼
                QR transport
                      │
                      ▼
             QR Reed-Solomon
                      │
                      ▼
                  Scanner
                      │
                      ▼
                CBOR decode
                      │
                      ▼
             Reassemble chunks
                      │
                      ▼
                Zstd decode
                      │
                      ▼
                example.pdf
                      │
                      ▼
                 SHA-256
                      │
                      ▼
                 ✓ verified
```

---

# 20. Design Principles

The initial CFT design follows these principles:

1. **CBOR carries structured metadata and binary data.**
2. **Zstd handles compression.**
3. **The compressed stream is chunked after compression.**
4. **Chunks are independently addressable.**
5. **The transfer is transport-independent.**
6. **The original file is protected by an end-to-end cryptographic hash.**
7. **Transport-specific error correction remains below CFT.**
8. **CBOR byte strings are used directly; base64 is unnecessary.**
9. **Protocol versions are explicit.**
10. **The format should remain small enough for constrained transports such as QR.**

---

# 21. Potential Future Extensions

A production version could add:

* Authentication / digital signatures
* Encryption
* Resumable transfers
* Chunk-level hashes
* Forward error correction above the transport
* Multiple compression algorithms
* Directory/archive transfers
* MIME/content-type metadata
* Creation/modification timestamps
* File permissions
* Transfer expiration
* Manifest signatures
* Multiple files in a single transfer

These should be extensions rather than requirements of the minimal format.

---

## Summary

The core idea is deliberately simple:

```text
             FILE
              │
             ZSTD
              │
       compressed bytes
              │
          ┌───┴───┐
          │       │
       chunk   chunk ...
          │       │
         CBOR    CBOR
          │       │
          └───┬───┘
              │
           TRANSPORT
              │
        ┌─────┴─────┐
        │           │
       QR          BLE
        │           │
        └─────┬─────┘
              │
          REASSEMBLE
              │
             ZSTD
              │
             FILE
              │
          SHA-256 ✓
```

The key separation is:

> **Zstd compresses the file, CBOR describes and carries it, the transport moves it, and the end-to-end hash verifies it.**