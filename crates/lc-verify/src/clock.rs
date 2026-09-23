//! Where `exp` is checked against (§10.6).
//!
//! > The verifier clock is now the only control, and it is load-bearing. [...] A verifier
//! > that cannot establish a trustworthy time **MUST NOT** fall back to accepting packets
//! > regardless of `exp`; failing closed is the required behaviour. — §10.6
//!
//! That requirement is why [`Clock::now`] returns `Option` rather than `u64`. A device
//! with no RTC, a clock that has not synchronised since boot, or one the application has
//! reason to distrust returns `None`, and the verifier answers `NoTrustedClock`. There is
//! no way to express "I don't know the time, accept anyway", which is the point.

/// A source of wall-clock time the verifier is willing to trust.
pub trait Clock {
    /// Seconds since the Unix epoch, or `None` if no trustworthy time is available.
    fn now(&self) -> Option<u64>;
}

/// A clock that refuses to answer. Every signed packet fails as `NoTrustedClock`.
///
/// The right default for a device that has not established time yet — it fails closed
/// without the caller having to remember to.
#[derive(Copy, Clone, Debug, Default)]
pub struct NoClock;

impl Clock for NoClock {
    fn now(&self) -> Option<u64> {
        None
    }
}

/// A fixed instant. For tests, and for callers that obtain time by their own means.
#[derive(Copy, Clone, Debug)]
pub struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn now(&self) -> Option<u64> {
        Some(self.0)
    }
}

/// The host clock, via `SystemTime`.
///
/// Note what this does *not* do: it reports whatever the operating system believes,
/// with no attestation that the OS is right. §10.6 asks verifiers to sanity-check their
/// clock against `iat` and against their own key material's issuance metadata — that
/// check belongs to the deployment, which knows what its key material says, so it is not
/// attempted here.
#[cfg(feature = "std")]
#[derive(Copy, Clone, Debug, Default)]
pub struct SystemClock;

#[cfg(feature = "std")]
impl Clock for SystemClock {
    fn now(&self) -> Option<u64> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    }
}
