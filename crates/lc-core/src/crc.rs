//! Frame CRCs (§9.1).
//!
//! Two parameterisations, chosen by frame format:
//!
//! * **CRC-16/CCITT-FALSE** — poly `0x1021`, init `0xFFFF`, no reflection, no xor-out.
//!   Used by the compact frame.
//! * **CRC-32/ISO-HDLC** — poly `0xEDB88320` reflected, init/xor-out `0xFFFFFFFF`.
//!   Used by the extended frame.
//!
//! ## Pinned: what the CRC covers
//!
//! §9.1 draws the frame as `header | body | CRC` without saying how a CRC over a
//! non-byte-aligned prefix is defined — the compact header is 15 bits, so `header|body`
//! is never byte-aligned. This implementation pins it:
//!
//! > The CRC input is the `header || body` bit sequence, **zero-padded to the next byte
//! > boundary**, hashed as bytes.
//!
//! The padding is 1 bit for the compact frame and 3 bits for the extended frame, and is
//! constant for a given frame format, so it adds no ambiguity and lets both ends use a
//! stock byte-wise CRC.

/// CRC-16/CCITT-FALSE over whole bytes.
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// CRC-32/ISO-HDLC (the zlib/PNG parameterisation) over whole bytes.
pub fn crc32_iso_hdlc(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    // The standard check vectors for both parameterisations: CRC of b"123456789".
    #[test]
    fn check_vectors() {
        assert_eq!(crc16_ccitt(b"123456789"), 0x29B1);
        assert_eq!(crc32_iso_hdlc(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_input_is_the_init_value() {
        assert_eq!(crc16_ccitt(b""), 0xFFFF);
        assert_eq!(crc32_iso_hdlc(b""), 0);
    }
}
