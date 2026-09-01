//! The tick clock.
//!
//! A tick is a ppm-second: one part per million of annual rate, held for one
//! second. `ticks(t)` is the exact integral of the administered rate since
//! module deployment, in integers, with no rounding anywhere.
//!
//! Everything here is integer arithmetic. The contract's `ticksAnchor` and
//! `anchorTime` are private with no getters, so the series is reconstructed
//! from the RateChanged logs; the constructor emits the first one, which makes
//! the reconstruction self anchoring at ticks = 0.

use serde::Serialize;

/// 365 days in seconds. The contract's fixed year, with no leap adjustment.
pub const YEAR_SECONDS: u128 = 31_536_000;
pub const PPM: u128 = 1_000_000;
/// The deployed AbstractLeadrate evaluates (timeNow - anchorTime) *
/// currentRatePPM in uint40, so one rate segment cannot exceed this many
/// ppm-seconds without reverting on chain.
pub const UINT40_MAX: u128 = (1u128 << 40) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RateSegment {
    /// Block timestamp of the RateChanged log that opened this segment.
    pub start: u64,
    pub rate_ppm: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TickClock {
    segments: Vec<RateSegment>,
}

impl TickClock {
    /// Segments must be in ascending start order and non-empty. The first
    /// segment's start is the module deployment timestamp, where ticks = 0.
    pub fn new(segments: Vec<RateSegment>) -> Result<Self, String> {
        if segments.is_empty() {
            return Err("the rate series is empty, so the tick clock has no origin".to_string());
        }
        for pair in segments.windows(2) {
            if pair[1].start <= pair[0].start {
                return Err(format!(
                    "the rate series is not strictly ascending: {} then {}",
                    pair[0].start, pair[1].start
                ));
            }
        }
        Ok(Self { segments })
    }

    pub fn segments(&self) -> &[RateSegment] {
        &self.segments
    }

    pub fn origin(&self) -> u64 {
        self.segments[0].start
    }

    /// ticks(t) = sum over segments starting before t of
    /// rate_i * (min(t, next_start) - start_i). Exact, all integers.
    pub fn ticks(&self, t: u64) -> Result<u64, String> {
        let mut total: u128 = 0;
        for (index, segment) in self.segments.iter().enumerate() {
            if segment.start >= t {
                break;
            }
            let end = match self.segments.get(index + 1) {
                Some(next) => next.start.min(t),
                None => t,
            };
            total += (end - segment.start) as u128 * segment.rate_ppm as u128;
        }
        u64::try_from(total).map_err(|_| {
            format!("ticks({t}) = {total} does not fit in the contract's uint64 accumulator")
        })
    }

    /// The administered rate in force at t, by last observation carried
    /// forward, which is what the contract's `currentRatePPM` holds.
    pub fn rate_at(&self, t: u64) -> u64 {
        let mut rate = self.segments[0].rate_ppm;
        for segment in &self.segments {
            if segment.start <= t {
                rate = segment.rate_ppm;
            } else {
                break;
            }
        }
        rate
    }

    /// The virtual accrual start: the wall-clock time at which the tick clock
    /// reaches the account's anchor, so interest begins to flow. Returns None
    /// when the anchor lies beyond the last segment's reach at `not_after`,
    /// or when the anchor is zero (a deleted account never accrues until the
    /// next deposit).
    ///
    /// `ticks` is piecewise linear and strictly increasing while the rate is
    /// nonzero, so the solution is closed form. The result is rounded up to
    /// the next whole second, because interest is zero for every integer
    /// timestamp strictly before the exact crossing.
    pub fn virtual_accrual_start(&self, anchor: u64, not_after: u64) -> Option<u64> {
        if anchor == 0 {
            return None;
        }
        let anchor = anchor as u128;
        let mut accumulated: u128 = 0;
        for (index, segment) in self.segments.iter().enumerate() {
            if segment.rate_ppm == 0 {
                continue;
            }
            let end = match self.segments.get(index + 1) {
                Some(next) => next.start,
                None => u64::MAX,
            };
            let span = (end.saturating_sub(segment.start)) as u128;
            let reach = accumulated + span * segment.rate_ppm as u128;
            if reach >= anchor {
                let remaining = anchor - accumulated;
                let rate = segment.rate_ppm as u128;
                // Round up: at the floor the clock has not yet reached the
                // anchor, so interest there is still zero.
                let offset = remaining.div_ceil(rate);
                let t = segment.start as u128 + offset;
                return if t <= not_after as u128 {
                    Some(t as u64)
                } else {
                    None
                };
            }
            accumulated = reach;
        }
        None
    }

    /// The deployed uint40 evaluation bound, checked per segment. Reported,
    /// not silently assumed: a segment past the bound would have reverted on
    /// chain, so observing one means the reconstruction is wrong.
    pub fn uint40_violations(&self) -> Vec<(RateSegment, u128)> {
        let mut out = Vec::new();
        for pair in self.segments.windows(2) {
            let product = (pair[1].start - pair[0].start) as u128 * pair[0].rate_ppm as u128;
            if product > UINT40_MAX {
                out.push((pair[0], product));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four observed rate changes on module 0x27d9AD98.
    fn observed() -> TickClock {
        TickClock::new(vec![
            RateSegment { start: 1_747_891_715, rate_ppm: 30_000 },
            RateSegment { start: 1_765_387_379, rate_ppm: 40_000 },
            RateSegment { start: 1_770_732_311, rate_ppm: 37_500 },
            RateSegment { start: 1_774_638_431, rate_ppm: 35_000 },
        ])
        .unwrap()
    }

    /// The tick clock reproduces the on-chain currentTicks() at
    /// the pinned block 25853000 timestamp.
    #[test]
    fn tick_clock_matches_chain_at_1787911199() {
        assert_eq!(observed().ticks(1_787_911_199).unwrap(), 1_349_693_580_000);
    }

    #[test]
    fn ticks_at_the_origin_is_zero() {
        let clock = observed();
        assert_eq!(clock.ticks(clock.origin()).unwrap(), 0);
    }

    #[test]
    fn rate_lookup_is_last_observation_carried_forward() {
        let clock = observed();
        assert_eq!(clock.rate_at(1_747_891_715), 30_000);
        assert_eq!(clock.rate_at(1_765_387_378), 30_000);
        assert_eq!(clock.rate_at(1_765_387_379), 40_000);
        assert_eq!(clock.rate_at(1_787_911_199), 35_000);
    }

    /// The observed history stays inside the deployed uint40 bound. The
    /// longest segment is about 202 days at 3.0 percent.
    #[test]
    fn observed_history_respects_the_uint40_bound() {
        assert!(observed().uint40_violations().is_empty());
    }

    #[test]
    fn a_segment_past_the_uint40_bound_is_reported() {
        // 2^40 - 1 ppm-seconds is about 424 days at 30000 ppm.
        let clock = TickClock::new(vec![
            RateSegment { start: 0, rate_ppm: 30_000 },
            RateSegment { start: 40_000_000, rate_ppm: 35_000 },
        ])
        .unwrap();
        let violations = clock.uint40_violations();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].1 > UINT40_MAX);
    }

    #[test]
    fn virtual_accrual_start_inverts_the_clock() {
        let clock = observed();
        for t in [1_760_000_000u64, 1_768_000_000, 1_780_000_000] {
            let anchor = clock.ticks(t).unwrap();
            let recovered = clock.virtual_accrual_start(anchor, u64::MAX).unwrap();
            // Rounded up to the next whole second, so it lands on t or just
            // after when the crossing is not exactly on a second.
            assert!(recovered >= t.saturating_sub(1) && recovered <= t + 1, "t={t} recovered={recovered}");
            assert!(clock.ticks(recovered).unwrap() >= anchor);
        }
    }

    #[test]
    fn a_zero_anchor_has_no_accrual_start() {
        assert_eq!(observed().virtual_accrual_start(0, u64::MAX), None);
    }

    #[test]
    fn a_non_ascending_series_is_rejected() {
        assert!(TickClock::new(vec![
            RateSegment { start: 100, rate_ppm: 1 },
            RateSegment { start: 100, rate_ppm: 2 },
        ])
        .is_err());
    }
}
