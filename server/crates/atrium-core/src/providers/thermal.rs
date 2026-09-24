//! Temperatures from `/sys/class/hwmon` (plan §9.2).
//!
//! Each `hwmonN` chip has a `name` and `tempM_input` files in
//! millidegrees Celsius, optionally with `tempM_label`. Every readable input
//! is reported as its own sensor with its chip and label. A sensor is marked
//! `cpu` only when its chip is one the plan names as a CPU sensor —
//! `coretemp` (Intel), `k10temp` (AMD), `cpu_thermal` (Arm SoCs) — and those
//! come first; nothing is inferred from a zone count or a name pattern.
//!
//! No readable sensor is an empty list **and** `temperatures` in
//! `unavailable` with `no_hwmon_sensors` (or `sysfs_unreadable` when sensors
//! exist and none could be read). No temperature is ever invented, and no
//! privileged read is used to find one.

use serde::Serialize;

use super::{read_line, safe_text, Absences, HostRoot, Reason};

/// Chips plan §9.2 names as CPU temperature sources.
pub const CPU_CHIPS: [&str; 3] = ["coretemp", "k10temp", "cpu_thermal"];
/// Chips examined at most.
const MAX_CHIPS: usize = 64;
/// Inputs examined per chip at most.
const MAX_INPUTS: u32 = 64;
/// Plausible range, in millidegrees; anything outside is a bad reading.
const PLAUSIBLE: std::ops::RangeInclusive<i64> = -100_000..=250_000;

/// One temperature reading.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sensor {
    /// The hwmon chip's `name`.
    pub chip: String,
    /// The input's label, when the driver provides one.
    pub label: Option<String>,
    /// Degrees Celsius, to 0.1.
    pub celsius: f64,
    /// The chip is a known CPU sensor.
    pub cpu: bool,
}

fn index_of(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.parse().ok()
}

/// Reads every sensor, recording `temperatures` as unavailable when none
/// can be read.
pub fn read(root: &HostRoot, absences: &mut Absences) -> Vec<Sensor> {
    let dir = root.path("sys/class/hwmon");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            absences.none::<()>("temperatures", Reason::NoHwmonSensors);
            return Vec::new();
        }
        Err(_) => {
            absences.none::<()>("temperatures", Reason::SysfsUnreadable);
            return Vec::new();
        }
    };
    let mut chips: Vec<u32> = entries
        .flatten()
        .filter_map(|entry| index_of(entry.file_name().to_str()?, "hwmon"))
        .collect();
    chips.sort_unstable();
    chips.truncate(MAX_CHIPS);

    let mut sensors = Vec::new();
    let mut seen_input = false;
    let mut malformed = false;
    for chip_index in chips {
        let base = format!("sys/class/hwmon/hwmon{chip_index}");
        let Some(chip) = read_line(&root.path(&format!("{base}/name")))
            .ok()
            .and_then(|name| safe_text(&name, 64))
        else {
            continue;
        };
        let Ok(files) = std::fs::read_dir(root.path(&base)) else {
            continue;
        };
        let mut inputs: Vec<u32> = files
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_str()?;
                index_of(name.strip_suffix("_input")?, "temp")
            })
            .filter(|index| *index <= MAX_INPUTS)
            .collect();
        inputs.sort_unstable();
        for input in inputs {
            seen_input = true;
            let Ok(raw) = read_line(&root.path(&format!("{base}/temp{input}_input"))) else {
                continue;
            };
            let Some(milli) = raw
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|m| PLAUSIBLE.contains(m))
            else {
                malformed = true;
                continue;
            };
            let label = read_line(&root.path(&format!("{base}/temp{input}_label")))
                .ok()
                .and_then(|label| safe_text(&label, 64));
            #[allow(clippy::cast_precision_loss)]
            let celsius = (milli as f64 / 100.0).round() / 10.0;
            sensors.push(Sensor {
                cpu: CPU_CHIPS.contains(&chip.as_str()),
                chip: chip.clone(),
                label,
                celsius,
            });
        }
    }
    // Known CPU sensors first; otherwise hwmon and input order.
    sensors.sort_by_key(|sensor| !sensor.cpu);
    if sensors.is_empty() {
        let reason = if malformed {
            Reason::SourceMalformed
        } else if seen_input {
            Reason::SysfsUnreadable
        } else {
            Reason::NoHwmonSensors
        };
        absences.none::<()>("temperatures", reason);
    }
    sensors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::fixture::Tree;
    use crate::providers::Unavailable;

    fn run(tree: &Tree) -> (Vec<Sensor>, Vec<Unavailable>) {
        let mut absences = Absences::default();
        let sensors = read(&tree.root(), &mut absences);
        (sensors, absences.into_vec())
    }

    #[test]
    fn sensors_are_read_labelled_and_cpu_chips_come_first() {
        let tree = Tree::new();
        tree.write("sys/class/hwmon/hwmon0/name", "nvme\n");
        tree.write("sys/class/hwmon/hwmon0/temp1_input", "38850\n");
        tree.write("sys/class/hwmon/hwmon0/temp1_label", "Composite\n");
        tree.write("sys/class/hwmon/hwmon1/name", "coretemp\n");
        tree.write("sys/class/hwmon/hwmon1/temp1_input", "52000\n");
        tree.write("sys/class/hwmon/hwmon1/temp1_label", "Package id 0\n");
        tree.write("sys/class/hwmon/hwmon1/temp2_input", "49000\n");
        tree.write("sys/class/hwmon/hwmon2/name", "acpitz\n");
        tree.write("sys/class/hwmon/hwmon2/temp1_input", "-5000\n");
        let (sensors, absent) = run(&tree);
        assert!(absent.is_empty());
        let summary: Vec<(&str, Option<&str>, f64, bool)> = sensors
            .iter()
            .map(|s| (s.chip.as_str(), s.label.as_deref(), s.celsius, s.cpu))
            .collect();
        assert_eq!(
            summary,
            [
                ("coretemp", Some("Package id 0"), 52.0, true),
                ("coretemp", None, 49.0, true),
                ("nvme", Some("Composite"), 38.9, false),
                // A single ACPI zone is not called a CPU sensor.
                ("acpitz", None, -5.0, false),
            ]
        );
    }

    #[test]
    fn no_sensor_is_unavailable_never_a_temperature() {
        let tree = Tree::new();
        let (sensors, absent) = run(&tree);
        assert!(sensors.is_empty());
        assert_eq!(absent[0].reason, Reason::NoHwmonSensors);

        // A chip with no temperature inputs (a fan controller, say).
        tree.write("sys/class/hwmon/hwmon0/name", "fan\n");
        tree.write("sys/class/hwmon/hwmon0/fan1_input", "1200\n");
        let (sensors, absent) = run(&tree);
        assert!(sensors.is_empty());
        assert_eq!(absent[0].reason, Reason::NoHwmonSensors);
    }

    #[test]
    fn implausible_readings_are_dropped_not_clamped() {
        let tree = Tree::new();
        tree.write("sys/class/hwmon/hwmon0/name", "odd\n");
        tree.write("sys/class/hwmon/hwmon0/temp1_input", "999999\n");
        tree.write("sys/class/hwmon/hwmon0/temp2_input", "hot\n");
        let (sensors, absent) = run(&tree);
        assert!(sensors.is_empty());
        assert_eq!(absent[0].reason, Reason::SourceMalformed);
    }
}
