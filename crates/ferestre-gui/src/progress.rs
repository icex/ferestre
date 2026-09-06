//! What a download is doing, in numbers a person reads.
//!
//! Split out from the window because none of it needs a toolkit: given a byte
//! count and the times it was seen at, this works out a rate and a time
//! remaining, and every awkward case -- a stalled download, a plan whose total
//! grows while it runs, the first second when there is nothing to average --
//! is a test rather than something noticed in front of a progress bar.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How far back the rate is averaged.
///
/// The instantaneous rate of a chunked download swings by an order of magnitude
/// between chunks, and an estimate computed from it alternates between "four
/// seconds" and "nine minutes" several times a second, which is worse than no
/// estimate at all. Twenty seconds is long enough to be steady and short enough
/// to notice the line actually slowing down.
const WINDOW: Duration = Duration::from_secs(20);

/// Below this there is not enough history to divide by, and a number computed
/// from two samples a hundred milliseconds apart is noise wearing a suit.
const MIN_SPAN: Duration = Duration::from_secs(2);

/// One download, as it is running.
#[derive(Debug, Clone)]
pub struct Download {
    /// Which title this is, so a row can say it is being downloaded rather than
    /// guessing from a directory that appears the moment a download starts.
    pub product_id: String,
    pub name: String,
    /// What the launcher is doing, for the line above the bar.
    pub what: String,
    pub done: u64,
    /// Not fixed: the client extends it as it walks the plan, so a bar that
    /// assumed the first total is one that goes backwards.
    pub total: u64,
    samples: VecDeque<(Instant, u64)>,
}

impl Download {
    pub fn new(product_id: &str, name: &str, what: &str, at: Instant) -> Self {
        Download {
            product_id: product_id.to_string(),
            name: name.to_string(),
            what: what.to_string(),
            done: 0,
            total: 0,
            samples: VecDeque::from([(at, 0)]),
        }
    }

    /// Record where the download has got to.
    pub fn observe(&mut self, done: u64, total: u64, at: Instant) {
        self.done = done;
        self.total = total;
        self.samples.push_back((at, done));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| at.duration_since(*t) > WINDOW)
        {
            // Keep the one that just fell outside: it is the far end of the
            // window, and dropping it leaves the average measuring a shorter
            // span than intended every time a sample expires.
            if self.samples.len() <= 2 {
                break;
            }
            self.samples.pop_front();
        }
    }

    /// `0.0` to `1.0`, or `None` while the total is still unknown.
    ///
    /// Not clamped to a rising value: the total really does grow mid-download,
    /// and a fraction that only ever increases would mean lying about one of
    /// the two numbers printed right beside it.
    pub fn fraction(&self) -> Option<f64> {
        (self.total > 0).then(|| (self.done as f64 / self.total as f64).clamp(0.0, 1.0))
    }

    /// Bytes per second over the window, or `None` when there is not enough
    /// history to say.
    pub fn rate(&self, now: Instant) -> Option<f64> {
        let (first_at, first_done) = *self.samples.front()?;
        let span = now.duration_since(first_at);
        if span < MIN_SPAN {
            return None;
        }
        let moved = self.done.checked_sub(first_done)?;
        Some(moved as f64 / span.as_secs_f64())
    }

    /// How long is left, or `None` when that cannot honestly be said.
    ///
    /// `None` covers three different situations and they are all the same
    /// answer: no total yet, not enough history, and a download that has moved
    /// nothing for the whole window. The last one is the important one -- a
    /// stalled transfer divides by zero, and the honest output is "stalled",
    /// not an hour that grows by an hour every hour.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        let rate = self.rate(now)?;
        if rate <= 0.0 {
            return None;
        }
        let left = self.total.checked_sub(self.done)?;
        if left == 0 {
            return Some(Duration::ZERO);
        }
        Some(Duration::from_secs_f64(
            (left as f64 / rate).min(u32::MAX as f64),
        ))
    }

    /// The line above the bar.
    pub fn heading(&self) -> String {
        format!("{} {}", self.what, self.name)
    }

    /// The line under the bar: how far, how fast, how long left.
    pub fn detail(&self, now: Instant) -> String {
        let mut parts = Vec::new();
        parts.push(match self.total {
            0 => bytes(self.done),
            total => format!("{} of {}", bytes(self.done), bytes(total)),
        });
        match self.rate(now) {
            Some(rate) if rate > 0.0 => parts.push(format!("{}/s", bytes(rate as u64))),
            // Said out loud rather than left as a gap. A download that has
            // moved nothing for twenty seconds is the case someone most needs
            // to be told about, and it is exactly when a bar looks fine.
            Some(_) => parts.push("stalled".into()),
            None => {}
        }
        if let Some(left) = self.remaining(now) {
            parts.push(format!("{} left", duration(left)));
        }
        parts.join("  ·  ")
    }
}

/// The same units the catalog sizes are shown in, from the same place: a
/// download that says 8.1 GB has to agree with the row that offered it.
pub use ferestre_core::human_bytes as bytes;

/// Rounded to something worth reading. "1h 12m" and not "1h 12m 4s": the
/// seconds are wrong by more than a second, and printing them claims otherwise.
pub fn duration(d: Duration) -> String {
    let total = d.as_secs();
    match total {
        0..=59 => format!("{total}s"),
        60..=3599 => match (total / 60, total % 60) {
            (m, 0) => format!("{m}m"),
            (m, s) => format!("{m}m {s}s"),
        },
        _ => match (total / 3600, (total % 3600) / 60) {
            (h, 0) => format!("{h}h"),
            (h, m) => format!("{h}h {m}m"),
        },
    }
}

/// One `{"progress":{"done":N,"total":M}}` line from the client.
///
/// Anything else on that stream is not an error and not progress: the client
/// prints its own messages there, and a line that is not this shape is simply
/// not an update.
pub fn parse_line(line: &str) -> Option<(u64, u64)> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let progress = value.get("progress")?;
    Some((
        progress.get("done")?.as_u64()?,
        progress.get("total")?.as_u64()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn a_line_of_progress_is_read_and_anything_else_is_ignored() {
        assert_eq!(
            parse_line(r#"{"progress":{"done":128,"total":4096}}"#),
            Some((128, 4096))
        );
        assert_eq!(
            parse_line("  {\"progress\":{\"done\":1,\"total\":2}}  "),
            Some((1, 2))
        );
        // The client's own output on the same stream is not an update.
        assert_eq!(parse_line(":: downloading 9NBLGGH2JHXJ"), None);
        assert_eq!(parse_line("{}"), None);
        assert_eq!(parse_line(r#"{"progress":{"done":1}}"#), None);
        assert_eq!(parse_line(""), None);
    }

    #[test]
    fn a_steady_download_reports_its_rate_and_what_is_left() {
        let start = Instant::now();
        let mut d = Download::new("9ZZTESTGAME1", "A Title", "Installing", start);
        // 10 MB/s for ten seconds, of a 200 MB download.
        for second in 1..=10 {
            d.observe(second * 10_000_000, 200_000_000, at(start, second));
        }
        let now = at(start, 10);
        let rate = d.rate(now).expect("ten seconds is enough history");
        assert!(
            (rate - 10_000_000.0).abs() < 100_000.0,
            "about 10 MB/s, got {rate}"
        );
        // 100 MB left at 10 MB/s.
        assert_eq!(d.remaining(now), Some(Duration::from_secs(10)));
        assert_eq!(d.fraction(), Some(0.5));
        assert!(
            d.detail(now).contains("100 MB of 200 MB"),
            "{}",
            d.detail(now)
        );
        assert!(d.detail(now).contains("10s left"), "{}", d.detail(now));
    }

    /// The first moment of a download, which is most of what someone sees of a
    /// short one. A number invented from two samples a fraction of a second
    /// apart is noise, and printing it as a time remaining is worse than a gap.
    #[test]
    fn nothing_is_claimed_before_there_is_history_to_claim_it_from() {
        let start = Instant::now();
        let mut d = Download::new("9ZZTESTGAME1", "A Title", "Installing", start);
        d.observe(4_000_000, 200_000_000, start + Duration::from_millis(100));
        let now = start + Duration::from_millis(100);
        assert_eq!(d.rate(now), None);
        assert_eq!(d.remaining(now), None);
        // The bar still moves, and the byte counts are still real.
        assert_eq!(d.fraction(), Some(0.02));
        assert!(
            d.detail(now).contains("4 MB of 200 MB"),
            "{}",
            d.detail(now)
        );
        assert!(!d.detail(now).contains("left"), "{}", d.detail(now));
    }

    /// A stalled transfer divides by zero. The honest answer is to say it has
    /// stalled -- which is exactly when a progress bar looks fine and tells
    /// nobody anything.
    #[test]
    fn a_stalled_download_says_so_instead_of_estimating_forever() {
        let start = Instant::now();
        let mut d = Download::new("9ZZTESTGAME1", "A Title", "Installing", start);
        d.observe(50_000_000, 200_000_000, at(start, 1));
        for second in 2..=30 {
            d.observe(50_000_000, 200_000_000, at(start, second));
        }
        let now = at(start, 30);
        assert_eq!(d.rate(now), Some(0.0));
        assert_eq!(d.remaining(now), None, "no honest estimate exists");
        assert!(d.detail(now).contains("stalled"), "{}", d.detail(now));
    }

    /// The client extends the total as it walks the plan, so the fraction goes
    /// down. Clamping it upward would mean the bar disagreeing with the two
    /// numbers printed directly beneath it.
    #[test]
    fn a_total_that_grows_is_reported_rather_than_smoothed_over() {
        let start = Instant::now();
        let mut d = Download::new("9ZZTESTGAME1", "A Title", "Installing", start);
        d.observe(90_000_000, 100_000_000, at(start, 1));
        assert_eq!(d.fraction(), Some(0.9));
        d.observe(90_000_000, 300_000_000, at(start, 2));
        assert_eq!(d.fraction(), Some(0.3));
    }

    #[test]
    fn there_is_no_progress_to_show_before_a_total_arrives() {
        let start = Instant::now();
        let mut d = Download::new("9ZZTESTGAME1", "A Title", "Installing", start);
        d.observe(1_000_000, 0, at(start, 1));
        assert_eq!(d.fraction(), None, "an indeterminate bar, not a full one");
        assert_eq!(d.detail(at(start, 1)), "1 MB");
    }

    #[test]
    fn the_heading_says_what_is_happening_to_what() {
        let d = Download::new("9ZZTESTGAME1", "A Title", "Installing", Instant::now());
        assert_eq!(d.heading(), "Installing A Title");
    }

    #[test]
    fn sizes_and_durations_read_the_way_people_say_them() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(45_780_000_000), "45.8 GB");
        assert_eq!(bytes(2_500_000), "2 MB");

        assert_eq!(duration(Duration::from_secs(9)), "9s");
        assert_eq!(duration(Duration::from_secs(60)), "1m");
        assert_eq!(duration(Duration::from_secs(125)), "2m 5s");
        assert_eq!(duration(Duration::from_secs(3600)), "1h");
        assert_eq!(duration(Duration::from_secs(4320)), "1h 12m");
    }
}
