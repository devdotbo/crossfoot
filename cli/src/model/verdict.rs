//! Comparison of the model against chain state, and the run verdict.
//!
//! Tolerance is zero by design. This product class is fully recomputable from
//! public inputs, so a nonzero residual is a finding, not noise. Any
//! relaxation has to be argued in the result rather than assumed here.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    /// Every compared value equal to the wei.
    ModelMatch,
    /// At least one nonzero residual.
    ObservedDeviation,
    /// Inputs could not be fetched at the pinned blocks, but cached or later
    /// state exists.
    SourceStale,
    /// A required series is unobtainable.
    InputGap,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::ModelMatch => "MODEL_MATCH",
            Verdict::ObservedDeviation => "OBSERVED_DEVIATION",
            Verdict::SourceStale => "SOURCE_STALE",
            Verdict::InputGap => "INPUT_GAP",
        }
    }
}

/// One compared quantity. Values are decimal strings because they exceed
/// what JSON numbers represent exactly.
#[derive(Debug, Clone, Serialize)]
pub struct FieldComparison {
    pub field: String,
    pub modeled: String,
    pub observed: String,
    /// modeled minus observed, as a signed decimal string.
    pub residual: String,
    pub equal: bool,
}

impl FieldComparison {
    pub fn new(field: &str, modeled: u128, observed: u128) -> Self {
        let residual = if modeled >= observed {
            (modeled - observed).to_string()
        } else {
            format!("-{}", observed - modeled)
        };
        Self {
            field: field.to_string(),
            modeled: modeled.to_string(),
            observed: observed.to_string(),
            residual,
            equal: modeled == observed,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ComparisonSet {
    pub check_class: &'static str,
    pub tolerance: &'static str,
    pub fields: Vec<FieldComparison>,
}

impl ComparisonSet {
    pub fn new(fields: Vec<FieldComparison>) -> Self {
        Self {
            check_class: "full recomputation",
            tolerance: "zero, to the wei",
            fields,
        }
    }

    pub fn all_equal(&self) -> bool {
        self.fields.iter().all(|field| field.equal)
    }

    pub fn deviations(&self) -> Vec<&FieldComparison> {
        self.fields.iter().filter(|field| !field.equal).collect()
    }
}

/// Locates the first recognition event after which the model and the chain
/// disagree, using the per-event InterestCollected observations. Returns None
/// when every event agreed, which means a final-state deviation came from
/// something other than the recognised interest series.
pub fn first_divergence(interest_mismatches: &[Value]) -> Option<Value> {
    interest_mismatches.first().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_exact_match_has_a_zero_residual() {
        let field = FieldComparison::new("saved", 100, 100);
        assert!(field.equal);
        assert_eq!(field.residual, "0");
    }

    #[test]
    fn residual_sign_shows_the_direction() {
        assert_eq!(FieldComparison::new("a", 105, 100).residual, "5");
        assert_eq!(FieldComparison::new("a", 95, 100).residual, "-5");
        assert!(!FieldComparison::new("a", 95, 100).equal);
    }

    #[test]
    fn a_set_is_equal_only_when_every_field_is() {
        let set = ComparisonSet::new(vec![
            FieldComparison::new("a", 1, 1),
            FieldComparison::new("b", 2, 3),
        ]);
        assert!(!set.all_equal());
        assert_eq!(set.deviations().len(), 1);
        assert_eq!(set.deviations()[0].field, "b");
    }

    #[test]
    fn verdict_names_are_the_spec_names() {
        assert_eq!(Verdict::ModelMatch.as_str(), "MODEL_MATCH");
        assert_eq!(Verdict::ObservedDeviation.as_str(), "OBSERVED_DEVIATION");
        assert_eq!(Verdict::SourceStale.as_str(), "SOURCE_STALE");
        assert_eq!(Verdict::InputGap.as_str(), "INPUT_GAP");
        assert_eq!(json!(Verdict::ModelMatch), json!("MODEL_MATCH"));
    }
}
