//! Memory and swap from `/proc/meminfo` (plan §9.2).
//!
//! Only these keys are read: `MemTotal`, `MemAvailable`, `MemFree`,
//! `Buffers`, `Cached`, `SwapTotal`, `SwapFree`. Each is `<n> kB`, and a kB
//! here is 1024 bytes (the kernel's own convention in this file). A key that
//! is absent is `not_reported` — except `MemAvailable`, whose absence is
//! `mem_available_unsupported`, and which is never estimated from the other
//! fields. A value that does not parse, or overflows on conversion to bytes,
//! is `source_malformed`. No swap configured (`SwapTotal: 0 kB`) is
//! `swap_absent`, distinct from an unreadable file.

use serde::Serialize;

use super::{read_source, reason_for, Absences, HostRoot, Reason, Sourced, SMALL_SOURCE};

/// `memory` in `GET /api/v1/system/metrics`. Bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    /// `MemTotal`.
    pub total_bytes: Option<u64>,
    /// `MemAvailable`.
    pub available_bytes: Option<u64>,
    /// `MemFree`.
    pub free_bytes: Option<u64>,
    /// `Buffers`.
    pub buffers_bytes: Option<u64>,
    /// `Cached`.
    pub cached_bytes: Option<u64>,
}

/// `swap`. Both `null` with `swap_absent` when no swap is configured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Swap {
    /// `SwapTotal`.
    pub total_bytes: Option<u64>,
    /// `SwapFree`.
    pub free_bytes: Option<u64>,
}

/// One key's value in bytes, `None` when the key is absent.
fn value(text: &str, key: &str) -> Result<Option<u64>, Reason> {
    let mut found = None;
    for line in text.lines().take(256) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name != key {
            continue;
        }
        let mut parts = rest.split_whitespace();
        let number: u64 = parts
            .next()
            .and_then(|n| n.parse().ok())
            .ok_or(Reason::SourceMalformed)?;
        if parts.next() != Some("kB") || parts.next().is_some() {
            return Err(Reason::SourceMalformed);
        }
        found = Some(number.checked_mul(1024).ok_or(Reason::SourceMalformed)?);
    }
    Ok(found)
}

fn field(text: &Sourced<String>, key: &str, absent: Reason) -> Sourced<u64> {
    let text = text.as_ref().map_err(|reason| *reason)?;
    value(text, key)?.ok_or(absent)
}

/// Reads memory and swap, recording each absence under `memory.*` and
/// `swap.*`.
pub fn read(root: &HostRoot, absences: &mut Absences) -> (Memory, Swap) {
    let text = read_source(&root.path("proc/meminfo"), SMALL_SOURCE)
        .map_err(|failure| reason_for(failure, Reason::ProcfsUnreadable));
    let mut take =
        |name: &str, key: &str, absent: Reason| absences.take(name, field(&text, key, absent));
    let memory = Memory {
        total_bytes: take("memory.totalBytes", "MemTotal", Reason::NotReported),
        available_bytes: take(
            "memory.availableBytes",
            "MemAvailable",
            Reason::MemAvailableUnsupported,
        ),
        free_bytes: take("memory.freeBytes", "MemFree", Reason::NotReported),
        buffers_bytes: take("memory.buffersBytes", "Buffers", Reason::NotReported),
        cached_bytes: take("memory.cachedBytes", "Cached", Reason::NotReported),
    };
    let swap = match field(&text, "SwapTotal", Reason::NotReported) {
        Ok(0) => Swap {
            total_bytes: absences.none("swap.totalBytes", Reason::SwapAbsent),
            free_bytes: absences.none("swap.freeBytes", Reason::SwapAbsent),
        },
        Ok(total) => Swap {
            total_bytes: Some(total),
            free_bytes: absences.take(
                "swap.freeBytes",
                field(&text, "SwapFree", Reason::NotReported),
            ),
        },
        Err(reason) => Swap {
            total_bytes: absences.none("swap.totalBytes", reason),
            free_bytes: absences.none("swap.freeBytes", reason),
        },
    };
    (memory, swap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::fixture::Tree;
    use crate::providers::Unavailable;

    const MEMINFO: &str = "MemTotal:       16305560 kB\nMemFree:         1234560 kB\n\
        MemAvailable:   10000000 kB\nBuffers:          200000 kB\nCached:          5000000 kB\n\
        SwapCached:            0 kB\nSwapTotal:       2097148 kB\nSwapFree:        2097148 kB\n";

    fn run(content: Option<&str>) -> (Memory, Swap, Vec<Unavailable>) {
        let tree = Tree::new();
        if let Some(content) = content {
            tree.write("proc/meminfo", content);
        }
        let mut absences = Absences::default();
        let (memory, swap) = read(&tree.root(), &mut absences);
        (memory, swap, absences.into_vec())
    }

    #[test]
    fn values_are_kib_times_1024() {
        let (memory, swap, absent) = run(Some(MEMINFO));
        assert!(absent.is_empty(), "{absent:?}");
        assert_eq!(memory.total_bytes, Some(16_305_560 * 1024));
        assert_eq!(memory.available_bytes, Some(10_000_000 * 1024));
        assert_eq!(swap.total_bytes, Some(2_097_148 * 1024));
    }

    #[test]
    fn no_mem_available_is_unsupported_and_never_estimated() {
        let (memory, _, absent) = run(Some(&MEMINFO.replace("MemAvailable:   10000000 kB\n", "")));
        assert_eq!(memory.available_bytes, None);
        assert!(absent.contains(&Unavailable {
            field: "memory.availableBytes".into(),
            reason: Reason::MemAvailableUnsupported
        }));
        assert_eq!(memory.free_bytes, Some(1_234_560 * 1024));
    }

    #[test]
    fn no_swap_differs_from_unreadable_swap() {
        let none = MEMINFO
            .replace("SwapTotal:       2097148 kB", "SwapTotal:       0 kB")
            .replace("SwapFree:        2097148 kB", "SwapFree:        0 kB");
        let (_, swap, absent) = run(Some(&none));
        assert_eq!(swap.total_bytes, None);
        assert!(absent.iter().any(|u| u.reason == Reason::SwapAbsent));

        let (memory, swap, absent) = run(None);
        assert_eq!((swap.total_bytes, memory.total_bytes), (None, None));
        assert!(absent.iter().all(|u| u.reason == Reason::ProcfsUnreadable));
        assert!(!absent.iter().any(|u| u.reason == Reason::SwapAbsent));
    }

    #[test]
    fn a_genuine_zero_stays_zero() {
        let (memory, _, _) = run(Some(
            &MEMINFO.replace("Buffers:          200000 kB", "Buffers:               0 kB"),
        ));
        assert_eq!(memory.buffers_bytes, Some(0));
    }

    #[test]
    fn malformed_and_overflowing_values_are_refused() {
        for bad in [
            "MemTotal: many kB\n",
            "MemTotal: 12 MB\n",
            "MemTotal: 12\n",
            "MemTotal: 12 kB extra\n",
            "MemTotal: 18446744073709551615 kB\n",
        ] {
            let (memory, _, absent) = run(Some(bad));
            assert_eq!(memory.total_bytes, None, "{bad:?}");
            assert!(absent.contains(&Unavailable {
                field: "memory.totalBytes".into(),
                reason: Reason::SourceMalformed
            }));
        }
    }
}
