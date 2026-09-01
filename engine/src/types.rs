use chrono::NaiveDateTime;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::fmt;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ContractType {
    PAM,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ContractRole {
    /// Real-position acquirer (long)
    RPA,
    /// Real-position liquidator (short)
    RPL,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum DayCountConvention {
    /// Actual/365 Fixed
    A365,
    /// Actual/360
    A360,
    /// 30E/360
    #[serde(rename = "30E360")]
    _30E360,
    /// Actual/Actual
    AA,
    /// Actual/365 Fixed at seconds resolution. Vendored extension, not an
    /// ACTUS convention: the year fraction is seconds(start, end) / 31536000,
    /// with no rounding of partial days. A365 shares the denominator but
    /// rounds the numerator up to whole days, which destroys the second
    /// resolution that on-chain accrual needs.
    A365S,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum EndOfMonthConvention {
    /// Same day
    SD,
    /// End of month
    EOM,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum BusinessDayConvention {
    NOS,
    SCF,
    SCMF,
    CSF,
    CSMF,
    SCP,
    SCMP,
    CSP,
    CSMP,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum EventType {
    /// Analysis/status
    AD,
    /// Initial exchange
    IED,
    /// Interest payment
    IP,
    /// Interest capitalization
    IPCI,
    /// Maturity
    MD,
    /// Purchase
    PRD,
    /// Termination
    TD,
    /// Rate reset
    RR,
    /// Principal redemption
    PR,
    /// Principal prepayment
    PP,
    /// Fee payment
    FP,
    /// Scaling
    SC,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

// ---------------------------------------------------------------------------
// Contract terms (input)
// ---------------------------------------------------------------------------

/// All PAM contract terms, deserialized from ACTUS JSON (camelCase).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractTerms {
    pub contract_type: ContractType,
    #[serde(rename = "contractID")]
    pub contract_id: String,
    #[serde(deserialize_with = "de::datetime")]
    pub status_date: NaiveDateTime,
    #[serde(default, deserialize_with = "de::datetime_opt")]
    pub contract_deal_date: Option<NaiveDateTime>,
    pub currency: String,
    #[serde(deserialize_with = "de::decimal_from_str")]
    pub notional_principal: Decimal,
    #[serde(deserialize_with = "de::datetime")]
    pub initial_exchange_date: NaiveDateTime,
    #[serde(deserialize_with = "de::datetime")]
    pub maturity_date: NaiveDateTime,
    #[serde(default, deserialize_with = "de::decimal_opt")]
    pub nominal_interest_rate: Option<Decimal>,
    #[serde(default, deserialize_with = "de::datetime_opt")]
    pub cycle_anchor_date_of_interest_payment: Option<NaiveDateTime>,
    pub cycle_of_interest_payment: Option<String>,
    pub day_count_convention: Option<DayCountConvention>,
    pub end_of_month_convention: Option<EndOfMonthConvention>,
    #[serde(
        default,
        rename = "premiumDiscountAtIED",
        deserialize_with = "de::decimal_opt"
    )]
    pub premium_discount_at_ied: Option<Decimal>,
    #[serde(default, deserialize_with = "de::decimal_opt")]
    pub rate_multiplier: Option<Decimal>,
    pub contract_role: ContractRole,

    // Rate reset
    #[serde(default, deserialize_with = "de::datetime_opt")]
    pub cycle_anchor_date_of_rate_reset: Option<NaiveDateTime>,
    pub cycle_of_rate_reset: Option<String>,
    #[serde(default, deserialize_with = "de::decimal_opt")]
    pub rate_spread: Option<Decimal>,
    pub market_object_code_of_rate_reset: Option<String>,

    // Purchase / termination
    #[serde(default, deserialize_with = "de::datetime_opt")]
    pub purchase_date: Option<NaiveDateTime>,
    #[serde(default, deserialize_with = "de::decimal_opt")]
    pub price_at_purchase_date: Option<Decimal>,
    #[serde(default, deserialize_with = "de::datetime_opt")]
    pub termination_date: Option<NaiveDateTime>,
    #[serde(default, deserialize_with = "de::decimal_opt")]
    pub price_at_termination_date: Option<Decimal>,

    // Interest capitalization
    #[serde(default, deserialize_with = "de::datetime_opt")]
    pub capitalization_end_date: Option<NaiveDateTime>,

    // Accrued interest (initial)
    #[serde(default, deserialize_with = "de::decimal_opt")]
    pub accrued_interest: Option<Decimal>,

    // Calendar / business day
    pub calendar: Option<String>,
    pub business_day_convention: Option<BusinessDayConvention>,
}

// ---------------------------------------------------------------------------
// State space
// ---------------------------------------------------------------------------

/// Contract state space -- tracks the evolving state between events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSpace {
    pub notional_principal: Decimal,
    pub nominal_interest_rate: Decimal,
    pub accrued_interest: Decimal,
    pub status_date: NaiveDateTime,
}

impl StateSpace {
    pub fn initial(terms: &ContractTerms) -> Self {
        let role_sign = match terms.contract_role {
            ContractRole::RPA => Decimal::ONE,
            ContractRole::RPL => Decimal::NEGATIVE_ONE,
        };

        let contract_active_at_status =
            terms.purchase_date.is_some() || terms.status_date >= terms.initial_exchange_date;

        if contract_active_at_status {
            return Self {
                notional_principal: role_sign * terms.notional_principal,
                nominal_interest_rate: terms.nominal_interest_rate.unwrap_or(Decimal::ZERO),
                accrued_interest: terms.accrued_interest.unwrap_or(Decimal::ZERO),
                status_date: if terms.purchase_date.is_some() {
                    terms.status_date.max(terms.initial_exchange_date)
                } else {
                    terms.status_date
                },
            };
        }

        Self {
            notional_principal: Decimal::ZERO,
            nominal_interest_rate: terms.nominal_interest_rate.unwrap_or(Decimal::ZERO),
            accrued_interest: Decimal::ZERO,
            status_date: terms.status_date,
        }
    }
}

// ---------------------------------------------------------------------------
// Scheduled event (intermediate, before payoff evaluation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ScheduledEvent {
    pub schedule_date: NaiveDateTime,
    pub event_date: NaiveDateTime,
    pub event_type: EventType,
}

// ---------------------------------------------------------------------------
// Contract event (output)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ContractEvent {
    pub schedule_date: NaiveDateTime,
    pub event_date: NaiveDateTime,
    pub event_type: EventType,
    pub payoff: Decimal,
    pub currency: String,
    pub notional_principal: Decimal,
    pub nominal_interest_rate: Decimal,
    pub accrued_interest: Decimal,
}

// ---------------------------------------------------------------------------
// Risk factor observations
// ---------------------------------------------------------------------------

/// External risk factor observations (e.g. market rates for rate resets).
#[derive(Debug, Clone, Default)]
pub struct RiskFactors {
    /// Map from market-object code to time-series of (timestamp, value).
    pub observations: std::collections::HashMap<String, Vec<(NaiveDateTime, Decimal)>>,
}

impl RiskFactors {
    /// Get the most recent observation at or before the given date.
    pub fn get_rate(&self, market_object_code: &str, date: NaiveDateTime) -> Option<Decimal> {
        self.observations
            .get(market_object_code)
            .and_then(|series| {
                series
                    .iter()
                    .filter(|(ts, _)| *ts <= date)
                    .max_by_key(|(ts, _)| *ts)
                    .map(|(_, v)| *v)
            })
    }
}

// ---------------------------------------------------------------------------
// Custom deserializers
// ---------------------------------------------------------------------------

mod de {
    use chrono::NaiveDateTime;
    use rust_decimal::Decimal;
    use serde::{self, Deserialize, Deserializer};
    use serde_json::Value;
    use std::str::FromStr;

    const FORMATS: &[&str] = &["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"];

    pub fn datetime<'de, D>(deserializer: D) -> Result<NaiveDateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        for fmt in FORMATS {
            if let Ok(dt) = NaiveDateTime::parse_from_str(&s, fmt) {
                return Ok(dt);
            }
        }
        Err(serde::de::Error::custom(format!(
            "cannot parse datetime: {s}"
        )))
    }

    pub fn datetime_opt<'de, D>(deserializer: D) -> Result<Option<NaiveDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(None),
            Some(s) => {
                for fmt in FORMATS {
                    if let Ok(dt) = NaiveDateTime::parse_from_str(&s, fmt) {
                        return Ok(Some(dt));
                    }
                }
                Err(serde::de::Error::custom(format!(
                    "cannot parse datetime: {s}"
                )))
            }
        }
    }

    /// Deserialize a Decimal from either a JSON string or a JSON number.
    fn decimal_from_value(v: &Value) -> Result<Decimal, String> {
        match v {
            Value::String(s) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return Err("empty string".into());
                }
                Decimal::from_str(trimmed).map_err(|e| e.to_string())
            }
            Value::Number(n) => {
                // Use the string representation of the JSON number for precision
                Decimal::from_str(&n.to_string()).map_err(|e| e.to_string())
            }
            _ => Err(format!("expected string or number, got {v:?}")),
        }
    }

    pub fn decimal_from_str<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
    where
        D: Deserializer<'de>,
    {
        let v = Value::deserialize(deserializer)?;
        decimal_from_value(&v).map_err(serde::de::Error::custom)
    }

    pub fn decimal_opt<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<Value> = Option::deserialize(deserializer)?;
        match opt {
            None => Ok(None),
            Some(Value::Null) => Ok(None),
            Some(Value::String(ref s)) if s.trim().is_empty() => Ok(None),
            Some(v) => decimal_from_value(&v)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}
