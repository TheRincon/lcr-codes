//! Error correction (§8.2).
//!
//! One quasi-cyclic LDPC family, soft-decision decoded, used for both layers at four
//! levels each with **unequal protection between the layers**. §8.2 calls that load
//! bearing and it is: chroma bits are measured over a lower-bandwidth, noisier channel
//! (§1.4), so equal rates would either waste luma capacity or under-protect chroma.

pub mod ldpc;
pub mod tables;
