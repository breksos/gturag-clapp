//! One update from a long-running job (an install, a download).
//!
//! A job reports two orthogonal things as it works: a human status *line* ("
//! extracting the Node runtime") and, while a determinate step runs, a *fraction*
//! in 0..=1 (a download that knows its total). Either may be absent: a plain
//! step is a line with no fraction; a download tick is a fraction with no new
//! line (it advances the bar under the line already shown). Carrying both in one
//! value lets the whole progress path (daemon channel, wire notification,
//! client callback, GUI) stay a single stream instead of two.

use serde::{Deserialize, Serialize};

/// A single progress update: an optional status line, an optional 0..=1
/// fraction, or both. An empty update (neither field) is a no-op.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// A human status line, when this update introduces a new step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// How far the current determinate step has come, in 0..=1. Absent means
    /// indeterminate (the caller shows a spinner, not a bar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f32>,
}

impl Progress {
    /// A download tick: `done`/`total` bytes as a fraction under the current
    /// line. An unknown total (`0`, no `Content-Length`) yields no fraction, so
    /// a lengthless download stays a spinner rather than a lying bar.
    pub fn frac(done: u64, total: u64) -> Self {
        Self {
            line: None,
            fraction: (total > 0).then(|| (done as f32 / total as f32).clamp(0.0, 1.0)),
        }
    }
}

impl From<String> for Progress {
    fn from(line: String) -> Self {
        Self {
            line: Some(line),
            fraction: None,
        }
    }
}

impl From<&str> for Progress {
    fn from(line: &str) -> Self {
        Self::from(line.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Progress;

    #[test]
    fn frac_is_the_ratio_and_clamps() {
        assert_eq!(Progress::frac(0, 100).fraction, Some(0.0));
        assert_eq!(Progress::frac(50, 100).fraction, Some(0.5));
        assert_eq!(Progress::frac(100, 100).fraction, Some(1.0));
        // A count past the total (a Content-Length that undercounts) never
        // reports over 1.0, so a bar cannot overflow.
        assert_eq!(Progress::frac(150, 100).fraction, Some(1.0));
    }

    #[test]
    fn frac_of_unknown_total_is_indeterminate() {
        // No Content-Length (total 0) means a spinner, never a lying bar.
        assert_eq!(Progress::frac(42, 0).fraction, None);
    }

    #[test]
    fn a_line_carries_no_fraction() {
        assert_eq!(Progress::from("resolving").fraction, None);
        assert_eq!(
            Progress::from("resolving").line.as_deref(),
            Some("resolving")
        );
    }
}
