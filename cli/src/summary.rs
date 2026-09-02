//! The target-neutral `summary` block of `result.json`.
//!
//! Every target writes the same key set, so the renderer's index row and the
//! consumer agent can read a result without target-specific code. The block
//! is derived from the verdict and the comparison the target already
//! computed; it never decides anything on its own.

use serde::Serialize;

use crate::model::mtbill::{CheckResult, CheckVerdict};
use crate::model::verdict::{ComparisonSet, Verdict};

/// A posted or recomputed quantity, as a decimal string with its scale.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Quantity {
    pub field: String,
    pub value: String,
    /// Null for a quantity that is a sentence rather than a number, such
    /// as the Midas survey line.
    pub decimals: Option<u32>,
}

/// The largest absolute residual of a deviating run, with the field it sits on.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Residual {
    pub field: String,
    pub residual: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Window {
    /// Null for a target that reads one pinned block, such as midas.
    pub baseline_block: Option<u64>,
    pub block: u64,
}

/// Field order is the serialisation order and matches the documented shape.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub target: String,
    pub family: &'static str,
    pub check_class: &'static str,
    pub nav_recomputation: &'static str,
    pub verdict: String,
    pub consumer_action: &'static str,
    pub headline: String,
    pub fields_compared: usize,
    pub fields_exact: usize,
    pub largest_residual: Option<Residual>,
    pub posted: Option<Quantity>,
    pub recomputed: Option<Quantity>,
    pub window: Window,
    pub findings_count: usize,
}

/// The consumer action is `ALLOW` on the passing verdict and `REVIEW` on
/// every other word. No target emits `REFUSE`: a finding does not prove the
/// posted value wrong.
pub fn consumer_action(passes: bool) -> &'static str {
    if passes {
        "ALLOW"
    } else {
        "REVIEW"
    }
}

/// Magnitude of a signed decimal string, for ordering residuals. The
/// values are differences of u128 quantities, so they fit in a u128.
fn magnitude(residual: &str) -> u128 {
    residual
        .trim_start_matches('-')
        .parse::<u128>()
        .unwrap_or(u128::MAX)
}

/// The svZCHF summary: a full recomputation compared field by field.
pub fn svzchf(
    verdict: Verdict,
    comparison: &ComparisonSet,
    window: Window,
    findings_count: usize,
) -> Summary {
    exact(
        "svzchf",
        "vault.price()",
        "recognised interest series deviates",
        verdict,
        comparison,
        window,
        findings_count,
    )
}

/// The sUSDe summary: the same shape, posted and recomputed being the
/// vault's convertToAssets(1e18).
pub fn susde(
    verdict: Verdict,
    comparison: &ComparisonSet,
    window: Window,
    findings_count: usize,
) -> Summary {
    exact(
        "susde",
        "vault.convertToAssets(1e18)",
        "reward series deviates",
        verdict,
        comparison,
        window,
        findings_count,
    )
}

/// A full recomputation compared field by field: the shared shape of the
/// recomputable-accrual targets.
fn exact(
    target: &str,
    posted_field: &str,
    series_note: &str,
    verdict: Verdict,
    comparison: &ComparisonSet,
    window: Window,
    findings_count: usize,
) -> Summary {
    let total = comparison.fields.len();
    let deviating = comparison.deviations();
    let exact = total - deviating.len();
    let largest_residual = deviating
        .iter()
        .max_by_key(|field| magnitude(&field.residual))
        .map(|field| Residual {
            field: field.field.clone(),
            residual: field.residual.clone(),
        });
    let headline = match verdict {
        Verdict::ModelMatch => format!("{exact} of {total} fields exact, residual 0"),
        Verdict::ObservedDeviation if deviating.is_empty() => {
            format!("0 of {total} fields deviate, {series_note}")
        }
        Verdict::ObservedDeviation => format!("{} of {total} fields deviate", deviating.len()),
        other => format!("{}, {exact} of {total} fields exact", other.as_str()),
    };
    let price = comparison
        .fields
        .iter()
        .find(|field| field.field == posted_field);
    Summary {
        target: target.to_string(),
        family: "recomputable-accrual",
        check_class: "full recomputation",
        nav_recomputation: "FULL",
        verdict: verdict.as_str().to_string(),
        consumer_action: consumer_action(verdict == Verdict::ModelMatch),
        headline,
        fields_compared: total,
        fields_exact: exact,
        largest_residual,
        posted: price.map(|field| Quantity {
            field: field.field.clone(),
            value: field.observed.clone(),
            decimals: Some(18),
        }),
        recomputed: price.map(|field| Quantity {
            field: field.field.clone(),
            value: field.modeled.clone(),
            decimals: Some(18),
        }),
        window,
        findings_count,
    }
}

/// The mTBILL summary: a consistency bundle over the issuer's own rules. The
/// NAV is never recomputed, so `recomputed` is null and `nav_recomputation`
/// is `INPUT_GAP`; the compared "fields" are the checks that carry a verdict.
pub fn mtbill(
    overall: &str,
    checks: &[CheckResult],
    latest_answer: i128,
    feed_decimals: u32,
    window: Window,
    findings_count: usize,
) -> Summary {
    let with_verdict: Vec<&CheckResult> = checks
        .iter()
        .filter(|check| check.verdict != CheckVerdict::Informational)
        .collect();
    let total = with_verdict.len();
    let exact = with_verdict
        .iter()
        .filter(|check| check.verdict == CheckVerdict::Consistent)
        .count();
    let failing = with_verdict
        .iter()
        .filter(|check| check.verdict == CheckVerdict::ObservedDeviation)
        .count();
    let violations: usize = checks.iter().map(|check| check.violations.len()).sum();
    let headline = match overall {
        "CONSISTENT" => format!("{exact} of {total} checks consistent"),
        "OBSERVED_DEVIATION" => {
            format!("{violations} violation(s) across {failing} failing check(s)")
        }
        other => format!("{other}, {exact} of {total} checks consistent"),
    };
    Summary {
        target: "mtbill".to_string(),
        family: "guarded-setter",
        check_class: "consistency",
        nav_recomputation: "INPUT_GAP",
        verdict: overall.to_string(),
        consumer_action: consumer_action(overall == "CONSISTENT"),
        headline,
        fields_compared: total,
        fields_exact: exact,
        largest_residual: None,
        posted: Some(Quantity {
            field: "oracle.latestAnswer()".to_string(),
            value: latest_answer.to_string(),
            decimals: Some(feed_decimals),
        }),
        recomputed: None,
        window,
        findings_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::verdict::FieldComparison;
    use serde_json::{json, Value};

    fn comparison(price_modeled: u128, assets_modeled: u128) -> ComparisonSet {
        ComparisonSet::new(vec![
            FieldComparison::new("account.saved", 1000, 1000),
            FieldComparison::new("account.ticks", 5, 5),
            FieldComparison::new("vault.totalAssets()", assets_modeled, 2000),
            FieldComparison::new("vault.price()", price_modeled, 1_021_764_268_673_581_424),
            FieldComparison::new(
                "vault.convertToAssets(1e18)",
                price_modeled,
                1_021_764_268_673_581_424,
            ),
        ])
    }

    /// The key set, sorted: serde_json orders object keys, so the set is
    /// what the contract fixes, not the order.
    fn keys(value: &Value) -> Vec<String> {
        let mut keys: Vec<String> = value
            .as_object()
            .expect("the summary is an object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    fn expected_keys() -> Vec<String> {
        let mut keys: Vec<String> = EXPECTED_KEYS.iter().map(|k| k.to_string()).collect();
        keys.sort();
        keys
    }

    const EXPECTED_KEYS: [&str; 14] = [
        "target",
        "family",
        "check_class",
        "nav_recomputation",
        "verdict",
        "consumer_action",
        "headline",
        "fields_compared",
        "fields_exact",
        "largest_residual",
        "posted",
        "recomputed",
        "window",
        "findings_count",
    ];

    /// R3 to R6: one key set for every target, both verdict branches.
    #[test]
    fn summary_block_is_target_neutral() {
        let window = Window {
            baseline_block: Some(24_570_000),
            block: 25_853_000,
        };

        let exact = svzchf(
            Verdict::ModelMatch,
            &comparison(1_021_764_268_673_581_424, 2000),
            window.clone(),
            0,
        );
        let exact_json = json!(exact);
        assert_eq!(keys(&exact_json), expected_keys());
        assert_eq!(exact.headline, "5 of 5 fields exact, residual 0");
        assert_eq!(exact.nav_recomputation, "FULL");
        assert_eq!(exact.consumer_action, "ALLOW");
        assert_eq!(exact.fields_compared, 5);
        assert_eq!(exact.fields_exact, 5);
        assert_eq!(exact.largest_residual, None);
        assert_eq!(
            exact_json["posted"],
            json!({"field": "vault.price()", "value": "1021764268673581424", "decimals": 18})
        );
        assert_eq!(exact_json["recomputed"], exact_json["posted"]);
        assert_eq!(
            exact_json["window"],
            json!({"baseline_block": 24_570_000, "block": 25_853_000})
        );

        // Two fields off: the price by one wei, the assets by three.
        let deviating = svzchf(
            Verdict::ObservedDeviation,
            &comparison(1_021_764_268_673_581_423, 1997),
            window.clone(),
            2,
        );
        let deviating_json = json!(deviating);
        assert_eq!(keys(&deviating_json), expected_keys());
        assert_eq!(deviating.headline, "3 of 5 fields deviate");
        assert_eq!(deviating.consumer_action, "REVIEW");
        assert_eq!(deviating.fields_exact, 2);
        assert_eq!(
            deviating.largest_residual,
            Some(Residual {
                field: "vault.totalAssets()".to_string(),
                residual: "-3".to_string(),
            })
        );
        assert_eq!(deviating.findings_count, 2);
        assert_eq!(deviating_json["recomputed"]["value"], "1021764268673581423");

        // The other verdicts are never ALLOW, whatever the residuals say.
        for verdict in [
            Verdict::ModelInconsistent,
            Verdict::SourceStale,
            Verdict::InputGap,
        ] {
            let summary = svzchf(
                verdict,
                &comparison(1_021_764_268_673_581_424, 2000),
                window.clone(),
                1,
            );
            assert_eq!(summary.consumer_action, "REVIEW");
            assert!(summary.headline.starts_with(verdict.as_str()));
        }

        // mTBILL: the same keys, the NAV never recomputed.
        let checks = vec![
            CheckResult::synthetic("C1", CheckVerdict::ObservedDeviation, 2),
            CheckResult::synthetic("C2", CheckVerdict::Consistent, 0),
            CheckResult::synthetic("C8", CheckVerdict::Informational, 0),
        ];
        let mtbill_deviating = mtbill(
            "OBSERVED_DEVIATION",
            &checks,
            103_373_777,
            8,
            window.clone(),
            0,
        );
        let mtbill_json = json!(mtbill_deviating);
        assert_eq!(keys(&mtbill_json), expected_keys());
        assert_eq!(mtbill_deviating.nav_recomputation, "INPUT_GAP");
        assert_eq!(mtbill_deviating.consumer_action, "REVIEW");
        assert_eq!(
            mtbill_deviating.headline,
            "2 violation(s) across 1 failing check(s)"
        );
        assert_eq!(mtbill_deviating.fields_compared, 2);
        assert_eq!(mtbill_deviating.fields_exact, 1);
        assert_eq!(mtbill_json["recomputed"], Value::Null);
        assert_eq!(mtbill_json["largest_residual"], Value::Null);
        assert_eq!(
            mtbill_json["posted"],
            json!({"field": "oracle.latestAnswer()", "value": "103373777", "decimals": 8})
        );

        let consistent = vec![
            CheckResult::synthetic("C1", CheckVerdict::Consistent, 0),
            CheckResult::synthetic("C8", CheckVerdict::Informational, 0),
        ];
        let mtbill_passing = mtbill("CONSISTENT", &consistent, 1, 8, window, 0);
        assert_eq!(mtbill_passing.consumer_action, "ALLOW");
        assert_eq!(mtbill_passing.headline, "1 of 1 checks consistent");
    }
}
