//! Per-source limits on the unauthenticated surface (M1E).
//!
//! Two small mechanisms, both keyed by the **accepted socket's** peer address
//! and nothing a client can say about itself — no `X-Forwarded-For`, no
//! `Forwarded`, no header of any kind:
//!
//! - [`ConnectionLimiter`]: at most [`MAX_CONNECTIONS_PER_SOURCE`] concurrent
//!   connections per source, inside the global cap of
//!   [`super::MAX_CONNECTIONS`]. One address can hold a quarter of the slots,
//!   so taking them all needs four sources that each hold eight open.
//! - [`RateLimiter`]: token buckets for the pairing routes, one per source
//!   and one global. They bound how fast anyone can make Core parse a pairing
//!   body, start a session or check a proof. The cryptographic limit is
//!   separate and authoritative: five wrong proofs lock the arming
//!   (`crate::pairing`).
//!
//! **What a source is.** IPv4 addresses are sources as they are. An
//! IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) is the same source as
//! `a.b.c.d`, so a dual-stack socket cannot be used to double a budget. An
//! IPv6 address is a source as it is. Grouping by /64, the usual choice for
//! a service on the Internet, is wrong for a server on a home LAN: every
//! host on the link shares one /64, so the whole network would be one
//! source, any neighbour could replace the owner's pairing attempt, and all
//! IPv6 clients would share eight connections. A host that claims many
//! addresses — IPv6 or IPv4 alike — gets one share per address, and the
//! global caps (32 connections, the global pairing budget, four attempts)
//! bound what any number of addresses can take.
//!
//! **Bounded state.** The connection map holds only sources with a live
//! connection, so never more than the global cap. The rate map holds at most
//! [`MAX_TRACKED_SOURCES`] entries; a full bucket carries no information and
//! is dropped first, and if every tracked source is still mid-budget a new
//! source is refused until one refills — failing closed, never growing.
//!
//! The limits and their reasons:
//!
//! | Limit | Value | Why |
//! | --- | --- | --- |
//! | connections per source | 8 | a browser opens ~6; a native client 1–2 |
//! | pairing requests per source | burst 10, then 10 a minute | one attempt is three requests; plan §8.2's 10/minute |
//! | pairing requests, all sources | burst 30, then 60 a minute | bounds work regardless of how many addresses an attacker has |
//! | tracked sources | 1024 | a few tens of KiB at most |

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Concurrent connections from one source.
pub const MAX_CONNECTIONS_PER_SOURCE: usize = 8;
/// Sources whose pairing budget is tracked at once.
pub const MAX_TRACKED_SOURCES: usize = 1024;

/// Where a connection comes from, as far as limits are concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKey {
    /// An IPv4 address, including an IPv4-mapped IPv6 one.
    V4(u32),
    /// An IPv6 address that is not IPv4-mapped.
    V6(u128),
}

impl SourceKey {
    /// The source of a peer address.
    #[must_use]
    pub fn of(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(v4) => Self::V4(u32::from(v4)),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => Self::V4(u32::from(v4)),
                None => Self::V6(u128::from(v6)),
            },
        }
    }
}

/// Counts live connections per source.
#[derive(Debug)]
pub struct ConnectionLimiter {
    per_source: usize,
    live: Mutex<HashMap<SourceKey, usize>>,
}

impl ConnectionLimiter {
    /// At most `per_source` concurrent connections from one source.
    #[must_use]
    pub fn new(per_source: usize) -> Arc<Self> {
        Arc::new(Self {
            per_source,
            live: Mutex::new(HashMap::new()),
        })
    }

    /// A slot for one more connection from `source`, or `None` when that
    /// source already has its share. The slot is returned on drop.
    #[must_use]
    pub fn try_acquire(self: &Arc<Self>, source: SourceKey) -> Option<ConnectionSlot> {
        let mut live = self.live.lock().ok()?;
        let count = live.entry(source).or_insert(0);
        if *count >= self.per_source {
            return None;
        }
        *count += 1;
        Some(ConnectionSlot {
            limiter: Arc::clone(self),
            source,
        })
    }

    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.live.lock().map(|live| live.len()).unwrap_or(0)
    }
}

/// One connection's claim on its source's share.
#[derive(Debug)]
pub struct ConnectionSlot {
    limiter: Arc<ConnectionLimiter>,
    source: SourceKey,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        if let Ok(mut live) = self.limiter.live.lock() {
            if let Some(count) = live.get_mut(&self.source) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    live.remove(&self.source);
                }
            }
        }
    }
}

/// A token bucket's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    /// Requests allowed at once.
    pub burst: u32,
    /// One more token every this long.
    pub every: Duration,
}

impl Rate {
    /// `burst` at once, then `per_minute` a minute.
    #[must_use]
    pub const fn per_minute(burst: u32, per_minute: u32) -> Self {
        Self {
            burst,
            every: Duration::from_millis(60_000 / per_minute as u64),
        }
    }
}

/// The pairing routes' budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingRates {
    /// Per source.
    pub per_source: Rate,
    /// For everyone together.
    pub global: Rate,
}

impl PairingRates {
    /// The production values; see the module table.
    pub const DEFAULT: Self = Self {
        per_source: Rate::per_minute(10, 10),
        global: Rate::per_minute(30, 60),
    };
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: u32,
    updated: Instant,
}

impl Bucket {
    fn full(rate: Rate, now: Instant) -> Self {
        Self {
            tokens: rate.burst,
            updated: now,
        }
    }

    fn refill(&mut self, rate: Rate, now: Instant) {
        let every = rate.every.as_nanos().max(1);
        let elapsed = now.saturating_duration_since(self.updated).as_nanos();
        let earned = elapsed / every;
        if earned == 0 {
            return;
        }
        let tokens = u128::from(self.tokens).saturating_add(earned);
        self.tokens = u32::try_from(tokens.min(u128::from(rate.burst))).unwrap_or(rate.burst);
        if self.tokens >= rate.burst {
            self.updated = now;
        } else {
            // Keep the fraction of a token already earned.
            let used = rate.every * u32::try_from(earned).unwrap_or(u32::MAX);
            self.updated += used;
        }
    }

    fn is_full(&self, rate: Rate) -> bool {
        self.tokens >= rate.burst
    }

    fn wait(&self, rate: Rate, now: Instant) -> Duration {
        (self.updated + rate.every).saturating_duration_since(now)
    }
}

/// Per-source and global token buckets.
#[derive(Debug)]
pub struct RateLimiter {
    rates: PairingRates,
    state: Mutex<RateState>,
}

#[derive(Debug)]
struct RateState {
    global: Bucket,
    sources: HashMap<SourceKey, Bucket>,
}

impl RateLimiter {
    /// A limiter with `rates`, full at `now`.
    #[must_use]
    pub fn new(rates: PairingRates, now: Instant) -> Self {
        Self {
            rates,
            state: Mutex::new(RateState {
                global: Bucket::full(rates.global, now),
                sources: HashMap::new(),
            }),
        }
    }

    /// Takes one token from `source` and one from the global bucket, or
    /// neither. `Err` carries how long until trying again could succeed,
    /// rounded up to whole seconds; it says nothing about how many requests
    /// remain.
    ///
    /// # Errors
    ///
    /// When either bucket is empty, or the source table is full of sources
    /// that are all mid-budget.
    pub fn check(&self, source: SourceKey, now: Instant) -> Result<(), Duration> {
        let refused = |wait: Duration| {
            let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
            Err(Duration::from_secs(seconds.max(1)))
        };
        let Ok(mut state) = self.state.lock() else {
            return refused(self.rates.per_source.every);
        };
        let rates = self.rates;
        state.global.refill(rates.global, now);
        if !state.sources.contains_key(&source) && state.sources.len() >= MAX_TRACKED_SOURCES {
            state.sources.retain(|_, bucket| {
                bucket.refill(rates.per_source, now);
                !bucket.is_full(rates.per_source)
            });
            if state.sources.len() >= MAX_TRACKED_SOURCES {
                return refused(rates.per_source.every);
            }
        }
        let bucket = state
            .sources
            .entry(source)
            .or_insert_with(|| Bucket::full(rates.per_source, now));
        bucket.refill(rates.per_source, now);
        if bucket.tokens == 0 {
            let wait = bucket.wait(rates.per_source, now);
            return refused(wait);
        }
        if state.global.tokens == 0 {
            let wait = state.global.wait(rates.global, now);
            return refused(wait);
        }
        state.global.tokens -= 1;
        if let Some(bucket) = state.sources.get_mut(&source) {
            bucket.tokens -= 1;
        }
        Ok(())
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.state.lock().map(|s| s.sources.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn equivalent_address_forms_are_one_source() {
        let v4 = SourceKey::of(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)));
        let mapped = SourceKey::of(IpAddr::V6(Ipv4Addr::new(192, 168, 1, 20).to_ipv6_mapped()));
        assert_eq!(v4, mapped);
        assert_ne!(
            v4,
            SourceKey::of(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 21)))
        );

        // Two neighbours on one LAN share a /64 and are still two sources.
        let a: Ipv6Addr = "2001:db8:1:2::1".parse().expect("v6");
        let b: Ipv6Addr = "2001:db8:1:2::2".parse().expect("v6");
        let a_again: Ipv6Addr = "2001:db8:1:2:0:0:0:1".parse().expect("v6");
        assert_ne!(SourceKey::of(IpAddr::V6(a)), SourceKey::of(IpAddr::V6(b)));
        assert_eq!(
            SourceKey::of(IpAddr::V6(a)),
            SourceKey::of(IpAddr::V6(a_again))
        );
    }

    #[test]
    fn one_source_gets_its_share_and_no_more() {
        let limiter = ConnectionLimiter::new(MAX_CONNECTIONS_PER_SOURCE);
        let one = SourceKey::V4(1);
        let two = SourceKey::V4(2);
        let held: Vec<_> = (0..MAX_CONNECTIONS_PER_SOURCE)
            .map(|_| limiter.try_acquire(one).expect("within share"))
            .collect();
        assert!(limiter.try_acquire(one).is_none());
        let other = limiter
            .try_acquire(two)
            .expect("another source is unaffected");
        drop(held);
        drop(other);
        assert_eq!(limiter.tracked(), 0, "released slots leave nothing behind");
        assert!(limiter.try_acquire(one).is_some());
    }

    #[test]
    fn buckets_refuse_when_empty_and_refill_with_time() {
        let start = Instant::now();
        let rates = PairingRates {
            per_source: Rate::per_minute(3, 60),
            global: Rate::per_minute(100, 600),
        };
        let limiter = RateLimiter::new(rates, start);
        let source = SourceKey::V4(7);
        for _ in 0..3 {
            limiter.check(source, start).expect("burst");
        }
        let wait = limiter.check(source, start).expect_err("empty");
        assert_eq!(wait, Duration::from_secs(1));
        // Another source has its own budget.
        limiter
            .check(SourceKey::V4(8), start)
            .expect("other source");
        // One second later, exactly one more.
        let later = start + Duration::from_secs(1);
        limiter.check(source, later).expect("refilled");
        assert!(limiter.check(source, later).is_err());
    }

    #[test]
    fn the_global_bucket_bounds_every_source_together() {
        let start = Instant::now();
        let rates = PairingRates {
            per_source: Rate::per_minute(10, 10),
            global: Rate::per_minute(5, 5),
        };
        let limiter = RateLimiter::new(rates, start);
        for n in 0..5 {
            limiter
                .check(SourceKey::V4(n), start)
                .expect("global burst");
        }
        assert!(limiter.check(SourceKey::V4(99), start).is_err());
    }

    #[test]
    fn source_state_is_bounded() {
        let start = Instant::now();
        let limiter = RateLimiter::new(
            PairingRates {
                per_source: Rate::per_minute(2, 1),
                global: Rate::per_minute(u32::MAX / 2, 60_000),
            },
            start,
        );
        for n in 0..u32::try_from(MAX_TRACKED_SOURCES).expect("small") {
            limiter.check(SourceKey::V4(n), start).expect("tracked");
        }
        assert_eq!(limiter.tracked(), MAX_TRACKED_SOURCES);
        // Every tracked source is mid-budget: a new one is refused, and the
        // table does not grow.
        assert!(limiter.check(SourceKey::V4(u32::MAX), start).is_err());
        assert_eq!(limiter.tracked(), MAX_TRACKED_SOURCES);
        // Once they have refilled they carry no information and make room.
        let later = start + Duration::from_secs(120);
        limiter
            .check(SourceKey::V4(u32::MAX), later)
            .expect("room after refill");
        assert!(limiter.tracked() <= MAX_TRACKED_SOURCES);
    }
}
