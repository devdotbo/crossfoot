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
    /// The tool's two independent model paths (the integer transcription of
    /// the deployed state machine and the ACTUS engine) disagree with each
    /// other. The model is then not trustworthy for this window, so no
    /// statement about the chain is made, whatever the residuals say.
    ModelInconsistent,
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
            Verdict::ModelInconsistent => "MODEL_INCONSISTENT",
            Verdict::SourceStale => "SOURCE_STALE",
            Verdict::InputGap => "INPUT_GAP",
        }
    }
}

/// The inputs to the run verdict, each already reduced to a fact.
#[derive(Debug, Clone, Copy)]
pub struct VerdictInputs {
    /// A required series is unobtainable (result caps, unanchored rate
    /// series, incomplete log sweeps).
    pub input_gap: bool,
    /// A read at a pinned block reverted or came back empty although the
    /// source exists.
    pub stale_read: bool,
    /// The ACTUS path agreed with the reference replay at every compared
    /// point, including the horizon.
    pub model_paths_agree: bool,
    /// Every compared quantity at the horizon is equal to the wei.
    pub all_equal: bool,
    /// Every recognised interest amount in the window was reproduced.
    pub interest_series_clean: bool,
}

/// The one place the run verdict is decided.
///
/// Order of precedence: an unobtainable input outranks a stale read, which
/// outranks a disagreement between the tool's own model paths, which outranks
/// any comparison with the chain. A comparison is only meaningful when the
/// inputs were observed and the model agrees with itself.
pub fn aggregate(inputs: VerdictInputs) -> Verdict {
    if inputs.input_gap {
        Verdict::InputGap
    } else if inputs.stale_read {
        Verdict::SourceStale
    } else if !inputs.model_paths_agree {
        Verdict::ModelInconsistent
    } else if inputs.all_equal && inputs.interest_series_clean {
        Verdict::ModelMatch
    } else {
        Verdict::ObservedDeviation
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

    const CLEAN: VerdictInputs = VerdictInputs {
        input_gap: false,
        stale_read: false,
        model_paths_agree: true,
        all_equal: true,
        interest_series_clean: true,
    };

    #[test]
    fn a_clean_run_is_a_model_match() {
        assert_eq!(aggregate(CLEAN), Verdict::ModelMatch);
    }

    #[test]
    fn a_model_path_disagreement_never_passes_as_a_match() {
        // Fault injection: the chain comparison is perfect, the ACTUS path
        // disagrees with the reference replay. Before this aggregation
        // existed the run reported MODEL_MATCH here.
        let faulty = VerdictInputs { model_paths_agree: false, ..CLEAN };
        assert_eq!(aggregate(faulty), Verdict::ModelInconsistent);
        // And a residual on top does not turn it back into a chain finding.
        let faulty = VerdictInputs { all_equal: false, ..faulty };
        assert_eq!(aggregate(faulty), Verdict::ModelInconsistent);
    }

    #[test]
    fn residuals_and_series_mismatches_are_deviations() {
        assert_eq!(aggregate(VerdictInputs { all_equal: false, ..CLEAN }), Verdict::ObservedDeviation);
        assert_eq!(
            aggregate(VerdictInputs { interest_series_clean: false, ..CLEAN }),
            Verdict::ObservedDeviation
        );
    }

    #[test]
    fn observation_failures_outrank_everything() {
        let stale = VerdictInputs { stale_read: true, model_paths_agree: false, all_equal: false, ..CLEAN };
        assert_eq!(aggregate(stale), Verdict::SourceStale);
        let gap = VerdictInputs { input_gap: true, ..stale };
        assert_eq!(aggregate(gap), Verdict::InputGap);
    }
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
