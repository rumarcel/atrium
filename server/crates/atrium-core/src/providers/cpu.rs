//! CPU usage and core count (plan §9.2).
//!
//! Usage is a difference between two readings of `/proc/stat`'s aggregate
//! `cpu` line, so a single reading says nothing. A background [`Sampler`]
//! reads it every [`SAMPLE_INTERVAL`]; until two readings exist the value is
//! `null` with `first_sample_pending`, never 0 %. Requests read the last
//! computed value and never wait for a sample.
//!
//! Guarded against: a counter that goes backwards (a reset or a bad read —
//! the baseline is reseeded and the value is pending again, never negative),
//! no time having passed (no division by zero; the previous value stands),
//! malformed or overflowing fields (`source_malformed`), and a sampler that
//! has stopped (`sample_stale` once the last value is older than
//! [`STALE_AFTER`]). Hot-plugged CPUs change only the aggregate's rate of
//! increase, which the difference absorbs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use super::{read_line, read_source, reason_for, HostRoot, Reason, Sourced, LARGE_SOURCE};

/// How often the sampler reads `/proc/stat`.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
/// A value older than this is not served.
pub const STALE_AFTER: Duration = Duration::from_secs(10);
/// Largest CPU count accepted from `present` (the kernel's own maximum is
/// far below).
const MAX_CPUS: u32 = 65_536;

/// Cumulative busy and total time, in `USER_HZ` ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Times {
    /// Everything but idle and iowait.
    pub busy: u64,
    /// Everything, guest time excluded (already counted in user and nice).
    pub total: u64,
}

/// Parses the aggregate line of `/proc/stat`.
///
/// # Errors
///
/// `source_malformed` for anything but the documented form.
pub fn parse_stat(text: &str) -> Sourced<Times> {
    let line = text
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or(Reason::SourceMalformed)?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .map_err(|_| Reason::SourceMalformed)?;
    if fields.len() < 4 {
        return Err(Reason::SourceMalformed);
    }
    let get = |index: usize| fields.get(index).copied().unwrap_or(0);
    let idle = get(3).checked_add(get(4)).ok_or(Reason::SourceMalformed)?;
    let total = fields
        .iter()
        .try_fold(0_u64, |sum, value| sum.checked_add(*value))
        .ok_or(Reason::SourceMalformed)?;
    Ok(Times {
        busy: total - idle,
        total,
    })
}

/// Reads `/proc/stat` once.
///
/// # Errors
///
/// `procfs_unreadable` or `source_malformed`.
pub fn read_times(root: &HostRoot) -> Sourced<Times> {
    let text = read_source(&root.path("proc/stat"), LARGE_SOURCE)
        .map_err(|failure| reason_for(failure, Reason::ProcfsUnreadable))?;
    parse_stat(&text)
}

/// Counts a kernel CPU list (`0-3,6,8-9`).
fn count_cpu_list(text: &str) -> Option<u32> {
    let mut count: u32 = 0;
    for part in text.trim().split(',') {
        let (low, high) = match part.split_once('-') {
            Some((low, high)) => (low.parse::<u32>().ok()?, high.parse::<u32>().ok()?),
            None => {
                let single = part.parse::<u32>().ok()?;
                (single, single)
            }
        };
        if high < low || high >= MAX_CPUS {
            return None;
        }
        count = count.checked_add(high - low + 1)?;
    }
    (count > 0 && count <= MAX_CPUS).then_some(count)
}

/// Logical CPUs present, from `/sys/devices/system/cpu/present`.
///
/// # Errors
///
/// `sysfs_unreadable` or `source_malformed`.
pub fn cores(root: &HostRoot) -> Sourced<u32> {
    let line = read_line(&root.path("sys/devices/system/cpu/present"))
        .map_err(|failure| reason_for(failure, Reason::SysfsUnreadable))?;
    count_cpu_list(&line).ok_or(Reason::SourceMalformed)
}

#[derive(Debug, Default)]
struct State {
    baseline: Option<Times>,
    usage: Option<(f64, Instant)>,
    failure: Option<(Reason, Instant)>,
}

/// The CPU usage sampler's state. Shared by the sampling task (the only
/// writer) and request handlers (readers), behind one mutex held for a few
/// arithmetic operations.
#[derive(Debug)]
pub struct Sampler {
    root: HostRoot,
    state: Mutex<State>,
}

impl Sampler {
    /// A sampler that has not sampled yet.
    #[must_use]
    pub fn new(root: HostRoot) -> Arc<Self> {
        Arc::new(Self {
            root,
            state: Mutex::new(State::default()),
        })
    }

    /// Takes one reading now.
    pub fn sample(&self, now: Instant) {
        let reading = read_times(&self.root);
        self.record(reading, now);
    }

    fn record(&self, reading: Sourced<Times>, now: Instant) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let times = match reading {
            Ok(times) => times,
            Err(reason) => {
                state.failure = Some((reason, now));
                return;
            }
        };
        state.failure = None;
        let Some(previous) = state.baseline else {
            state.baseline = Some(times);
            return;
        };
        if times.total < previous.total || times.busy < previous.busy {
            // A reset or a bad reading: start again rather than report a
            // negative or enormous value.
            state.baseline = Some(times);
            state.usage = None;
            return;
        }
        let total = times.total - previous.total;
        let busy = times.busy - previous.busy;
        if total == 0 {
            // No tick elapsed; the previous value still describes the machine.
            return;
        }
        if busy > total {
            state.baseline = Some(times);
            state.usage = None;
            state.failure = Some((Reason::SourceMalformed, now));
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let percent = (busy as f64 / total as f64) * 100.0;
        state.usage = Some(((percent * 10.0).round() / 10.0, now));
        state.baseline = Some(times);
    }

    /// The last computed usage, in percent of all CPUs (0–100, one decimal).
    ///
    /// # Errors
    ///
    /// `first_sample_pending` before two readings, the read failure's reason
    /// when the latest reading failed, `sample_stale` when the value is older
    /// than [`STALE_AFTER`].
    pub fn usage(&self, now: Instant) -> Sourced<f64> {
        let state = self.state.lock().map_err(|_| Reason::SampleStale)?;
        if let Some((reason, _)) = state.failure {
            return Err(reason);
        }
        match state.usage {
            None => Err(Reason::FirstSamplePending),
            Some((_, at)) if now.saturating_duration_since(at) > STALE_AFTER => {
                Err(Reason::SampleStale)
            }
            Some((value, _)) => Ok(value),
        }
    }
}

/// Samples every [`SAMPLE_INTERVAL`] until `stop` becomes `true`. One task,
/// no history, and a stop request is honoured between samples without
/// waiting for the next tick.
pub async fn run(sampler: Arc<Sampler>, mut stop: watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return;
                }
            }
            _ = ticker.tick() => {
                // A read of /proc/stat is a few kilobytes from memory.
                sampler.sample(Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::fixture::Tree;

    const STAT: &str = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 100 0 50 800 50 0 0 0 0 0\nintr 1\n";

    #[test]
    fn stat_is_parsed_with_iowait_as_idle_and_guest_excluded() {
        assert_eq!(
            parse_stat(STAT),
            Ok(Times {
                busy: 150,
                total: 1000
            })
        );
        // Guest columns are not added again.
        assert_eq!(
            parse_stat("cpu 10 0 0 90 0 0 0 0 999 999\n"),
            Ok(Times {
                busy: 10,
                total: 100
            })
        );
        for bad in [
            "",
            "cpu0 1 2 3 4\n",
            "cpu 1 2 3\n",
            "cpu 1 x 3 4\n",
            "cpu -1 2 3 4\n",
            "cpu 18446744073709551615 1 0 0\n",
        ] {
            assert_eq!(parse_stat(bad), Err(Reason::SourceMalformed), "{bad:?}");
        }
    }

    #[test]
    fn the_first_sample_is_pending_never_zero() {
        let sampler = Sampler::new(HostRoot::system());
        let start = Instant::now();
        assert_eq!(sampler.usage(start), Err(Reason::FirstSamplePending));
        sampler.record(
            Ok(Times {
                busy: 100,
                total: 1000,
            }),
            start,
        );
        assert_eq!(sampler.usage(start), Err(Reason::FirstSamplePending));
        let later = start + SAMPLE_INTERVAL;
        sampler.record(
            Ok(Times {
                busy: 150,
                total: 1200,
            }),
            later,
        );
        assert_eq!(sampler.usage(later), Ok(25.0));
        // A truly idle interval is a real zero.
        let idle = later + SAMPLE_INTERVAL;
        sampler.record(
            Ok(Times {
                busy: 150,
                total: 1400,
            }),
            idle,
        );
        assert_eq!(sampler.usage(idle), Ok(0.0));
    }

    #[test]
    fn regressions_reseed_instead_of_producing_nonsense() {
        let sampler = Sampler::new(HostRoot::system());
        let t = Instant::now();
        sampler.record(
            Ok(Times {
                busy: 500,
                total: 1000,
            }),
            t,
        );
        sampler.record(
            Ok(Times {
                busy: 600,
                total: 1100,
            }),
            t + SAMPLE_INTERVAL,
        );
        assert_eq!(sampler.usage(t + SAMPLE_INTERVAL), Ok(100.0));
        // Counters went backwards: pending again, never negative.
        sampler.record(
            Ok(Times {
                busy: 10,
                total: 20,
            }),
            t + SAMPLE_INTERVAL * 2,
        );
        assert_eq!(
            sampler.usage(t + SAMPLE_INTERVAL * 2),
            Err(Reason::FirstSamplePending)
        );
        // No time passed: no division by zero, the value stands.
        sampler.record(
            Ok(Times {
                busy: 20,
                total: 40,
            }),
            t + SAMPLE_INTERVAL * 3,
        );
        sampler.record(
            Ok(Times {
                busy: 20,
                total: 40,
            }),
            t + SAMPLE_INTERVAL * 4,
        );
        assert_eq!(sampler.usage(t + SAMPLE_INTERVAL * 4), Ok(50.0));
        // Busy grew faster than total: malformed, not 150 %.
        sampler.record(
            Ok(Times {
                busy: 200,
                total: 60,
            }),
            t + SAMPLE_INTERVAL * 5,
        );
        assert_eq!(
            sampler.usage(t + SAMPLE_INTERVAL * 5),
            Err(Reason::SourceMalformed)
        );
    }

    #[test]
    fn failures_and_staleness_are_reported() {
        let sampler = Sampler::new(HostRoot::system());
        let t = Instant::now();
        sampler.record(
            Ok(Times {
                busy: 0,
                total: 100,
            }),
            t,
        );
        sampler.record(
            Ok(Times {
                busy: 50,
                total: 200,
            }),
            t + SAMPLE_INTERVAL,
        );
        sampler.record(Err(Reason::ProcfsUnreadable), t + SAMPLE_INTERVAL * 2);
        assert_eq!(
            sampler.usage(t + SAMPLE_INTERVAL * 2),
            Err(Reason::ProcfsUnreadable)
        );
        sampler.record(
            Ok(Times {
                busy: 100,
                total: 300,
            }),
            t + SAMPLE_INTERVAL * 3,
        );
        assert_eq!(sampler.usage(t + SAMPLE_INTERVAL * 3), Ok(50.0));
        assert_eq!(
            sampler.usage(t + SAMPLE_INTERVAL * 3 + STALE_AFTER + Duration::from_secs(1)),
            Err(Reason::SampleStale)
        );
    }

    #[test]
    fn cpu_lists_are_counted_strictly() {
        assert_eq!(count_cpu_list("0-7\n"), Some(8));
        assert_eq!(count_cpu_list("0"), Some(1));
        assert_eq!(count_cpu_list("0-3,6,8-9"), Some(7));
        for bad in ["", "3-1", "a", "0-", "0-70000", "0,,1"] {
            assert_eq!(count_cpu_list(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_sampler_reads_a_fixture_and_stops_promptly() {
        let tree = Tree::new();
        tree.write("proc/stat", STAT);
        let sampler = Sampler::new(tree.root());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let (stop, receiver) = watch::channel(false);
            let task = tokio::spawn(run(Arc::clone(&sampler), receiver));
            tokio::time::sleep(Duration::from_millis(50)).await;
            stop.send(true).expect("stop");
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .expect("stops without waiting for a tick")
                .expect("no panic");
        });
        // One reading so far: pending, not zero.
        assert_eq!(
            sampler.usage(Instant::now()),
            Err(Reason::FirstSamplePending)
        );
    }
}
