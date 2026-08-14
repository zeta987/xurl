//! Human-readable rendering of thread activity times.
//!
//! Query listings keep the raw epoch under `updated_at` for machine readers and
//! add a `last_active` rendering for people. Relative wording is only useful
//! while the gap is small — past [`RELATIVE_CUTOFF_DAYS`] it says nothing that
//! the date does not, so the date stands alone.

use chrono::{DateTime, Local, TimeZone};

/// Beyond this age a relative phrase stops helping the reader locate a thread.
const RELATIVE_CUTOFF_DAYS: i64 = 30;

const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;

/// Renders `epoch` for a human reader, in the machine's local time zone.
///
/// Recent times read as `3 hours ago (2026-08-15 02:33)`; anything older than
/// [`RELATIVE_CUTOFF_DAYS`] drops the relative half and returns the date alone.
/// Returns `None` when the value is not a representable local time.
#[must_use]
pub fn format_last_active(epoch: u64) -> Option<String> {
    let now = Local::now();
    format_last_active_at(epoch, now)
}

fn format_last_active_at(epoch: u64, now: DateTime<Local>) -> Option<String> {
    let seconds = i64::try_from(epoch).ok()?;
    let moment = Local.timestamp_opt(seconds, 0).single()?;
    let absolute = moment.format("%Y-%m-%d %H:%M").to_string();

    let elapsed = now.signed_duration_since(moment).num_seconds();
    match relative_phrase(elapsed) {
        Some(relative) => Some(format!("{relative} ({absolute})")),
        None => Some(absolute),
    }
}

/// Returns the relative wording for `elapsed` seconds, or `None` once the gap is
/// wide enough that only the absolute date is worth showing.
///
/// Times in the future are reported as `just now` rather than as a negative age;
/// a thread stamped slightly ahead of the clock is a clock skew, not a forecast.
fn relative_phrase(elapsed: i64) -> Option<String> {
    if elapsed < SECONDS_PER_MINUTE {
        return Some("just now".to_string());
    }
    if elapsed < SECONDS_PER_HOUR {
        return Some(plural(elapsed / SECONDS_PER_MINUTE, "minute"));
    }
    if elapsed < SECONDS_PER_DAY {
        return Some(plural(elapsed / SECONDS_PER_HOUR, "hour"));
    }
    let days = elapsed / SECONDS_PER_DAY;
    if days < RELATIVE_CUTOFF_DAYS {
        return Some(plural(days, "day"));
    }
    None
}

fn plural(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone};

    use super::{format_last_active, format_last_active_at};

    /// Builds a local time from an epoch, mirroring what the formatter parses.
    fn local_at(epoch: i64) -> chrono::DateTime<Local> {
        Local.timestamp_opt(epoch, 0).single().expect("local time")
    }

    #[test]
    fn renders_relative_and_absolute_together() {
        let stamp = 1_786_736_840;
        let rendered =
            format_last_active_at(stamp as u64, local_at(stamp + 3 * 60 * 60)).expect("rendered");
        assert!(rendered.starts_with("3 hours ago ("), "got {rendered}");
        assert!(rendered.ends_with(')'), "got {rendered}");
    }

    #[test]
    fn singular_unit_drops_the_plural_s() {
        let stamp = 1_786_736_840;
        let rendered =
            format_last_active_at(stamp as u64, local_at(stamp + 60 * 60)).expect("rendered");
        assert!(rendered.starts_with("1 hour ago ("), "got {rendered}");
    }

    #[test]
    fn very_recent_reads_as_just_now() {
        let stamp = 1_786_736_840;
        let rendered = format_last_active_at(stamp as u64, local_at(stamp + 5)).expect("rendered");
        assert!(rendered.starts_with("just now ("), "got {rendered}");
    }

    #[test]
    fn beyond_cutoff_shows_the_date_alone() {
        let stamp = 1_786_736_840;
        let rendered = format_last_active_at(stamp as u64, local_at(stamp + 40 * 24 * 60 * 60))
            .expect("rendered");
        assert!(!rendered.contains("ago"), "got {rendered}");
        assert!(!rendered.contains('('), "got {rendered}");
    }

    #[test]
    fn future_stamps_do_not_render_negative_ages() {
        let stamp = 1_786_736_840;
        let rendered =
            format_last_active_at(stamp as u64, local_at(stamp - 10 * 60)).expect("rendered");
        assert!(rendered.starts_with("just now ("), "got {rendered}");
    }

    #[test]
    fn formats_a_real_epoch_against_the_system_clock() {
        assert!(format_last_active(1_786_736_840).is_some());
    }
}
