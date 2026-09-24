//! Load averages from `/proc/loadavg` (plan §9.2).

use serde::Serialize;

use super::{read_line, reason_for, Absences, HostRoot, Reason, Sourced};

/// `load` in `GET /api/v1/system/metrics`: runnable tasks averaged over one,
/// five and fifteen minutes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Load {
    /// 1 minute.
    pub one: Option<f64>,
    /// 5 minutes.
    pub five: Option<f64>,
    /// 15 minutes.
    pub fifteen: Option<f64>,
}

fn parse(line: &str) -> Sourced<[f64; 3]> {
    let mut values = [0.0; 3];
    let mut fields = line.split_whitespace();
    for value in &mut values {
        let parsed: f64 = fields
            .next()
            .and_then(|f| f.parse().ok())
            .ok_or(Reason::SourceMalformed)?;
        if !parsed.is_finite() || parsed < 0.0 || parsed > 1.0e6 {
            return Err(Reason::SourceMalformed);
        }
        *value = parsed;
    }
    Ok(values)
}

/// Reads the three averages; one failure makes all three unavailable, since
/// they share one line.
pub fn read(root: &HostRoot, absences: &mut Absences) -> Load {
    let parsed = read_line(&root.path("proc/loadavg"))
        .map_err(|failure| reason_for(failure, Reason::ProcfsUnreadable))
        .and_then(|line| parse(&line));
    match parsed {
        Ok([one, five, fifteen]) => Load {
            one: Some(one),
            five: Some(five),
            fifteen: Some(fifteen),
        },
        Err(reason) => Load {
            one: absences.none("load.one", reason),
            five: absences.none("load.five", reason),
            fifteen: absences.none("load.fifteen", reason),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadavg_is_parsed_and_bad_input_refused() {
        assert_eq!(parse("0.52 0.58 0.59 1/467 12345"), Ok([0.52, 0.58, 0.59]));
        assert_eq!(parse("0.00 0.00 0.00 1/1 1"), Ok([0.0, 0.0, 0.0]));
        for bad in ["", "0.5 0.5", "a b c", "-1 0 0", "NaN 0 0", "inf 0 0"] {
            assert_eq!(parse(bad), Err(Reason::SourceMalformed), "{bad:?}");
        }
    }
}
