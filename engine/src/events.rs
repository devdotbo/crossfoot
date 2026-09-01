use chrono::{Datelike, NaiveDate, NaiveDateTime};

use crate::types::{BusinessDayConvention, ContractTerms, EndOfMonthConvention, ScheduledEvent};

fn is_business_day(date: NaiveDateTime) -> bool {
    !matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun)
}

fn shift_following(mut date: NaiveDateTime) -> NaiveDateTime {
    while !is_business_day(date) {
        date += chrono::Duration::days(1);
    }
    date
}

fn shift_preceding(mut date: NaiveDateTime) -> NaiveDateTime {
    while !is_business_day(date) {
        date -= chrono::Duration::days(1);
    }
    date
}

fn shift_modified_following(date: NaiveDateTime) -> NaiveDateTime {
    let shifted = shift_following(date);
    if shifted.month() != date.month() {
        shift_preceding(date)
    } else {
        shifted
    }
}

fn shift_modified_preceding(date: NaiveDateTime) -> NaiveDateTime {
    let shifted = shift_preceding(date);
    if shifted.month() != date.month() {
        shift_following(date)
    } else {
        shifted
    }
}

fn apply_bdc_shift(date: NaiveDateTime, bdc: Option<BusinessDayConvention>) -> NaiveDateTime {
    match bdc {
        None | Some(BusinessDayConvention::NOS) => date,
        Some(BusinessDayConvention::SCF | BusinessDayConvention::CSF) => shift_following(date),
        Some(BusinessDayConvention::SCMF | BusinessDayConvention::CSMF) => {
            shift_modified_following(date)
        }
        Some(BusinessDayConvention::SCP | BusinessDayConvention::CSP) => shift_preceding(date),
        Some(BusinessDayConvention::SCMP | BusinessDayConvention::CSMP) => {
            shift_modified_preceding(date)
        }
    }
}

/// Generate the full event schedule for a PAM contract.
///
/// This produces a time-ordered sequence of scheduled events (IED, IP, IPCI,
/// RR, PRD, TD, MD) based on the contract terms and cycle definitions.
/// Build the event schedule for a PAM contract.
pub fn generate_schedule(terms: &ContractTerms) -> Vec<ScheduledEvent> {
    generate_schedule_with_rr_dates(terms, &[])
}

/// As `generate_schedule`, plus one RR event at each supplied date.
///
/// The injected dates are used verbatim, with no business-day shift: they are
/// block timestamps, for which the correct business-day convention is none.
/// An injected RR landing exactly on `maturity_date` is dropped, mirroring
/// the rule the cyclic RR path already applies.
///
/// Events at or before `status_date` are dropped downstream as usual, so a
/// caller that wants the first rate change to fire must set `status_date`
/// strictly earlier than it.
pub fn generate_schedule_with_rr_dates(
    terms: &ContractTerms,
    rr_dates: &[NaiveDateTime],
) -> Vec<ScheduledEvent> {
    use crate::types::EventType;

    let mut schedule = Vec::new();

    // IED event at initial exchange date
    schedule.push(ScheduledEvent {
        schedule_date: terms.initial_exchange_date,
        event_date: terms.initial_exchange_date,
        event_type: EventType::IED,
    });

    if let Some(purchase_date) = terms.purchase_date {
        schedule.push(ScheduledEvent {
            schedule_date: purchase_date,
            event_date: purchase_date,
            event_type: EventType::PRD,
        });
    }

    // MD event at maturity date
    schedule.push(ScheduledEvent {
        schedule_date: terms.maturity_date,
        event_date: terms.maturity_date,
        event_type: EventType::MD,
    });

    if let Some(termination_date) = terms.termination_date {
        schedule.push(ScheduledEvent {
            schedule_date: termination_date,
            event_date: termination_date,
            event_type: EventType::TD,
        });
    }

    // IP events from cycle definition
    if let Some(ref cycle_str) = terms.cycle_of_interest_payment {
        if let Some(cycle) = parse_cycle(cycle_str) {
            let anchor = terms
                .cycle_anchor_date_of_interest_payment
                .unwrap_or(terms.initial_exchange_date);
            let eom = terms
                .end_of_month_convention
                .unwrap_or(EndOfMonthConvention::SD);

            let ip_dates = generate_ip_dates(anchor, &cycle, terms.maturity_date, eom);
            let capitalization_end_date = terms.capitalization_end_date;

            for date in &ip_dates {
                let event_type = if capitalization_end_date.is_some_and(|end| *date <= end) {
                    EventType::IPCI
                } else {
                    EventType::IP
                };

                schedule.push(ScheduledEvent {
                    schedule_date: *date,
                    event_date: apply_bdc_shift(*date, terms.business_day_convention),
                    event_type,
                });
            }

            if let Some(capitalization_end_date) = capitalization_end_date {
                if capitalization_end_date < terms.maturity_date
                    && !ip_dates.contains(&capitalization_end_date)
                {
                    schedule.push(ScheduledEvent {
                        schedule_date: capitalization_end_date,
                        event_date: apply_bdc_shift(
                            capitalization_end_date,
                            terms.business_day_convention,
                        ),
                        event_type: EventType::IPCI,
                    });
                }
            }
        }
    }

    if let (Some(anchor), Some(cycle_str)) = (
        terms.cycle_anchor_date_of_rate_reset,
        terms.cycle_of_rate_reset.as_deref(),
    ) {
        if let Some(cycle) = parse_cycle(cycle_str) {
            let eom = terms
                .end_of_month_convention
                .unwrap_or(EndOfMonthConvention::SD);
            let mut rr_dates = generate_ip_dates(anchor, &cycle, terms.maturity_date, eom);
            if rr_dates.last() == Some(&terms.maturity_date) {
                rr_dates.pop();
            }

            for date in rr_dates {
                schedule.push(ScheduledEvent {
                    schedule_date: date,
                    event_date: apply_bdc_shift(date, terms.business_day_convention),
                    event_type: EventType::RR,
                });
            }
        }
    }

    for date in rr_dates {
        if *date == terms.maturity_date {
            continue;
        }
        schedule.push(ScheduledEvent {
            schedule_date: *date,
            event_date: *date,
            event_type: EventType::RR,
        });
    }

    // Sort by event_date, then by event-type priority (IED before MD if same date)
    schedule.sort_by(|a, b| {
        a.event_date
            .cmp(&b.event_date)
            .then_with(|| event_type_order(a.event_type).cmp(&event_type_order(b.event_type)))
            .then_with(|| a.schedule_date.cmp(&b.schedule_date))
    });

    // Filter out events at or before the status date
    schedule.retain(|e| e.event_date > terms.status_date);

    if let Some(purchase_date) = terms.purchase_date {
        schedule.retain(|e| e.event_type != EventType::IED && e.event_date >= purchase_date);
    }

    if let Some(termination_date) = terms.termination_date {
        schedule.retain(|e| e.event_type != EventType::MD && e.event_date <= termination_date);
    }

    schedule
}

// ---------------------------------------------------------------------------
// IP date generation
// ---------------------------------------------------------------------------

/// Generate IP event dates from anchor through maturity.
///
/// Produces cycle dates from anchor, handles end-of-month convention, and
/// applies stub convention when maturity doesn't fall on a cycle date.
fn generate_ip_dates(
    anchor: NaiveDateTime,
    cycle: &Cycle,
    maturity: NaiveDateTime,
    eom: EndOfMonthConvention,
) -> Vec<NaiveDateTime> {
    let mut dates = Vec::new();
    let mut n = 0u32;

    loop {
        let date = advance_by_cycle(anchor, cycle, n, eom);
        if date >= maturity {
            if date == maturity {
                dates.push(date);
            }
            break;
        }
        dates.push(date);
        n += 1;
    }

    // If last date is maturity, no stub handling needed
    if dates.last() == Some(&maturity) {
        return dates;
    }

    // Stub handling: maturity doesn't fall on a cycle date
    match cycle.stub {
        StubConvention::Long => {
            // Long last stub: drop last cycle date, extend period to maturity
            if dates.len() > 1 {
                dates.pop();
            }
        }
        StubConvention::Short => {
            // Short last stub: keep all cycle dates, add short period to maturity
        }
    }
    dates.push(maturity);

    dates
}

/// Advance a date by n multiples of the cycle period.
fn advance_by_cycle(
    anchor: NaiveDateTime,
    cycle: &Cycle,
    n: u32,
    eom: EndOfMonthConvention,
) -> NaiveDateTime {
    let total = cycle.n * n;
    match cycle.period {
        CyclePeriod::Day => anchor + chrono::Duration::days(total as i64),
        CyclePeriod::Week => anchor + chrono::Duration::weeks(total as i64),
        CyclePeriod::Month => advance_months(anchor, total, eom),
        CyclePeriod::Quarter => advance_months(anchor, total * 3, eom),
        CyclePeriod::HalfYear => advance_months(anchor, total * 6, eom),
        CyclePeriod::Year => advance_months(anchor, total * 12, eom),
    }
}

/// Advance a date by the given number of months, respecting EOM convention.
fn advance_months(anchor: NaiveDateTime, months: u32, eom: EndOfMonthConvention) -> NaiveDateTime {
    if months == 0 {
        return anchor;
    }

    let anchor_day = anchor.day();
    let anchor_month_days = days_in_month(anchor.year(), anchor.month());
    let anchor_is_eom = anchor_day == anchor_month_days;

    let total_months_0 = (anchor.month() - 1) + months;
    let target_year = anchor.year() + (total_months_0 / 12) as i32;
    let target_month = total_months_0 % 12 + 1;
    let target_month_days = days_in_month(target_year, target_month);

    let day = if eom == EndOfMonthConvention::EOM && anchor_is_eom {
        // EOM: anchor was end-of-month, so target should be end-of-month
        target_month_days
    } else {
        // SD or anchor not end-of-month: use anchor day, clamped
        anchor_day.min(target_month_days)
    };

    NaiveDate::from_ymd_opt(target_year, target_month, day)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
}

/// Number of days in a given month.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => unreachable!(),
    }
}

/// Ordering priority for event types when they fall on the same date.
fn event_type_order(et: crate::types::EventType) -> u8 {
    use crate::types::EventType;
    match et {
        EventType::AD => 0,
        EventType::IED => 1,
        EventType::PR => 2,
        EventType::PP => 3,
        EventType::IP => 4,
        EventType::IPCI => 5,
        EventType::RR => 6,
        EventType::FP => 7,
        EventType::SC => 8,
        EventType::PRD => 9,
        EventType::TD => 10,
        EventType::MD => 11,
    }
}

// ---------------------------------------------------------------------------
// Cycle parsing
// ---------------------------------------------------------------------------

/// Parsed ACTUS cycle string, e.g. "P1ML0" -> Period(1, Month, Long stub).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    pub n: u32,
    pub period: CyclePeriod,
    pub stub: StubConvention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CyclePeriod {
    Day,
    Week,
    Month,
    Quarter,
    HalfYear,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StubConvention {
    /// Long last stub
    Long,
    /// Short last stub
    Short,
}

/// Parse an ACTUS cycle string like "P1ML0" or "P3ML1".
///
/// Format: P{n}{period}L{stub}
///   - n: integer count
///   - period: D(ay), W(eek), M(onth), Q(uarter), H(alf-year), Y(ear)
///   - stub: 0 = short, 1 = long
pub fn parse_cycle(s: &str) -> Option<Cycle> {
    let s = s.trim();
    if !s.starts_with('P') {
        return None;
    }
    let rest = &s[1..];

    // Find the period letter position
    let period_pos = rest.find(|c: char| c.is_ascii_alphabetic())?;
    let n: u32 = rest[..period_pos].parse().ok()?;
    let period_char = rest.as_bytes()[period_pos] as char;

    let period = match period_char {
        'D' => CyclePeriod::Day,
        'W' => CyclePeriod::Week,
        'M' => CyclePeriod::Month,
        'Q' => CyclePeriod::Quarter,
        'H' => CyclePeriod::HalfYear,
        'Y' => CyclePeriod::Year,
        _ => return None,
    };

    // Parse stub convention after 'L'
    // ACTUS: S=0 -> long last stub, S=1 -> short last stub
    let after_period = &rest[period_pos + 1..];
    let stub = if after_period.starts_with('L') {
        match after_period.get(1..2) {
            Some("0") => StubConvention::Long,
            Some("1") => StubConvention::Short,
            _ => StubConvention::Long, // default
        }
    } else {
        StubConvention::Long // default
    };

    Some(Cycle { n, period, stub })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_p1ml0() {
        let c = parse_cycle("P1ML0").unwrap();
        assert_eq!(c.n, 1);
        assert_eq!(c.period, CyclePeriod::Month);
        assert_eq!(c.stub, StubConvention::Long);
    }

    #[test]
    fn parse_p3ml1() {
        let c = parse_cycle("P3ML1").unwrap();
        assert_eq!(c.n, 3);
        assert_eq!(c.period, CyclePeriod::Month);
        assert_eq!(c.stub, StubConvention::Short);
    }

    #[test]
    fn parse_p1yl0() {
        let c = parse_cycle("P1YL0").unwrap();
        assert_eq!(c.n, 1);
        assert_eq!(c.period, CyclePeriod::Year);
        assert_eq!(c.stub, StubConvention::Long);
    }
}
