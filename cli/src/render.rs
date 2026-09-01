//! Static site generator over the evidence bundles.
//!
//! Plain HTML with inline CSS and inline SVG. No JavaScript, no bundler, no
//! CDN, no network at view time: the output opens from file:// and every
//! number on it comes from the bundle it links to.
//!
//! The output is a pure function of the bundles, so rendering twice gives
//! byte identical files. Nothing here reads the clock.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// One bundle that carries a result and can therefore be rendered.
struct Run {
    dir: PathBuf,
    name: String,
    result: Value,
    manifest: Option<Value>,
    result_sha256: String,
    /// The bundle root hash from bundle.sha256, the evidence hash a reader
    /// cites and `crossfoot verify` recomputes. Absent on an unsealed bundle.
    root_hash: Option<String>,
}

pub struct RenderOutcome {
    pub out_dir: PathBuf,
    pub pages: Vec<PathBuf>,
    pub rendered: usize,
    pub skipped: Vec<String>,
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    current.as_str()
}

fn u64_at(value: &Value, path: &[&str]) -> Option<u64> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    current.as_u64()
}

/// Unix seconds to an ISO 8601 UTC string, without pulling in a formatter at
/// view time.
fn utc(timestamp: u64) -> String {
    chrono::DateTime::from_timestamp(timestamp as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%SZ").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

/// A decimal string in wei to a readable fixed-point string, without losing
/// digits: the full integer is kept and a decimal point inserted.
fn wei(value: &str, decimals: usize) -> String {
    let negative = value.starts_with('-');
    let digits = value.trim_start_matches('-');
    if digits.len() <= decimals {
        let padded = format!("{:0>width$}", digits, width = decimals + 1);
        let split = padded.len() - decimals;
        return format!(
            "{}{}.{}",
            if negative { "-" } else { "" },
            &padded[..split],
            &padded[split..]
        );
    }
    let split = digits.len() - decimals;
    format!(
        "{}{}.{}",
        if negative { "-" } else { "" },
        &digits[..split],
        &digits[split..]
    )
}

// ---------------------------------------------------------------------------
// Page furniture
// ---------------------------------------------------------------------------

const STYLE: &str = r#"
:root { --ink: #1b1b1b; --muted: #5c5c5c; --rule: #d8d4cc; --paper: #faf9f6;
        --accent: #8a5a2b; --panel: #f2efe9; }
* { box-sizing: border-box; }
body { background: var(--paper); color: var(--ink); margin: 0;
       font-family: Georgia, "Times New Roman", serif; line-height: 1.5; }
main { max-width: 60rem; margin: 0 auto; padding: 2rem 1.25rem 4rem; }
h1 { font-size: 1.5rem; margin: 0 0 .25rem; letter-spacing: -.01em; }
h2 { font-size: 1.05rem; margin: 2rem 0 .5rem; padding-bottom: .25rem;
     border-bottom: 1px solid var(--rule); font-weight: normal;
     text-transform: uppercase; letter-spacing: .08em; color: var(--muted); }
h3 { font-size: .95rem; margin: 1.25rem 0 .35rem; font-weight: bold; }
p { margin: .5rem 0; }
a { color: var(--ink); }
.lede { color: var(--muted); font-size: .9rem; max-width: 46rem; }
table { border-collapse: collapse; width: 100%; margin: .5rem 0 1rem;
        font-size: .82rem; }
th, td { text-align: left; padding: .35rem .55rem; border-bottom: 1px solid var(--rule);
         vertical-align: top; }
th { font-weight: normal; color: var(--muted); text-transform: uppercase;
     letter-spacing: .06em; font-size: .7rem; white-space: nowrap; }
.num, code, .mono { font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
                    font-size: .78rem; word-break: break-all; }
td.num { text-align: right; white-space: nowrap; }
.verdict { font-family: ui-monospace, Menlo, monospace; font-size: .8rem; }
.flag { color: var(--accent); font-weight: bold; }
.note { color: var(--muted); font-size: .8rem; }
.panel { background: var(--panel); border: 1px solid var(--rule); padding: .75rem 1rem;
         margin: 1rem 0; font-size: .85rem; }
.kv { display: grid; grid-template-columns: 13rem 1fr; gap: .15rem .75rem;
      font-size: .82rem; }
.kv dt { color: var(--muted); }
.kv dd { margin: 0; font-family: ui-monospace, Menlo, monospace; font-size: .78rem;
         word-break: break-all; }
figure { margin: 1rem 0; }
figcaption { color: var(--muted); font-size: .78rem; margin-top: .35rem; }
svg { max-width: 100%; height: auto; background: #fff; border: 1px solid var(--rule); }
ul { font-size: .85rem; }
.back { font-size: .82rem; color: var(--muted); }
"#;

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{}</title>\n<style>{}</style>\n</head>\n<body>\n<main>\n{}</main>\n</body>\n</html>\n",
        escape(title),
        STYLE,
        body
    )
}

// ---------------------------------------------------------------------------
// Inline SVG
// ---------------------------------------------------------------------------

/// svZCHF: modeled against observed recognised interest per recognition
/// event, two overlaid series, with the administered rate path as a step line
/// beneath on its own axis.
fn svg_svzchf(result: &Value) -> Option<String> {
    let steps = result.get("replay_steps")?.as_array()?;
    let points: Vec<(u64, f64, Option<f64>)> = steps
        .iter()
        .filter_map(|step| {
            let timestamp = step.get("timestamp")?.as_u64()?;
            let modeled: f64 = step.get("modeled_interest")?.as_str()?.parse().ok()?;
            let observed = step
                .get("observed_interest")
                .and_then(Value::as_str)
                .and_then(|v| v.parse::<f64>().ok());
            Some((timestamp, modeled, observed))
        })
        .filter(|(_, modeled, observed)| *modeled > 0.0 || observed.is_some())
        .collect();
    if points.len() < 2 {
        return None;
    }

    let segments: Vec<(u64, u64)> = result
        .get("inputs")?
        .get("rate_segments")?
        .as_array()?
        .iter()
        .filter_map(|s| Some((s.get("start")?.as_u64()?, s.get("rate_ppm")?.as_u64()?)))
        .collect();

    let (w, h) = (900.0f64, 420.0f64);
    let (l, r, t) = (86.0f64, 18.0f64, 16.0f64);
    let top_h = 250.0f64;
    let rate_top = t + top_h + 46.0;
    let rate_h = 70.0f64;

    let t_min = points.first().unwrap().0 as f64;
    let t_max = points.last().unwrap().0 as f64;
    let span = (t_max - t_min).max(1.0);
    let y_max = points
        .iter()
        .flat_map(|(_, m, o)| [*m, o.unwrap_or(0.0)])
        .fold(0.0f64, f64::max)
        .max(1.0);

    let x = |ts: f64| l + (ts - t_min) / span * (w - l - r);
    let y = |v: f64| t + top_h - (v / y_max) * top_h;

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" xmlns=\"http://www.w3.org/2000/svg\" role=\"img\" aria-label=\"modeled against observed recognised interest per recognition event, with the administered rate path beneath\">"
    ));

    // Axes and gridlines for the interest panel.
    for i in 0..=4 {
        let value = y_max * i as f64 / 4.0;
        let yy = y(value);
        svg.push_str(&format!(
            "<line x1=\"{l}\" y1=\"{yy:.1}\" x2=\"{:.1}\" y2=\"{yy:.1}\" stroke=\"#e6e2da\" stroke-width=\"1\"/>",
            w - r
        ));
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\" font-size=\"9\" fill=\"#5c5c5c\" font-family=\"monospace\">{:.2}</text>",
            l - 6.0,
            yy + 3.0,
            value / 1e18
        ));
    }
    svg.push_str(&format!(
        "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"9\" fill=\"#5c5c5c\" transform=\"rotate(-90 {:.1} {:.1})\" font-family=\"monospace\">interest, ZCHF</text>",
        14.0, t + top_h / 2.0, 14.0, t + top_h / 2.0
    ));

    // Modeled series as a line, observed as hollow circles on top, so an
    // agreement reads as circles sitting exactly on the line.
    let path: Vec<String> = points
        .iter()
        .map(|(ts, m, _)| format!("{:.1},{:.1}", x(*ts as f64), y(*m)))
        .collect();
    svg.push_str(&format!(
        "<polyline fill=\"none\" stroke=\"#1b1b1b\" stroke-width=\"1.2\" points=\"{}\"/>",
        path.join(" ")
    ));
    for (ts, _, observed) in &points {
        if let Some(observed) = observed {
            svg.push_str(&format!(
                "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"3.1\" fill=\"none\" stroke=\"#8a5a2b\" stroke-width=\"1.2\"/>",
                x(*ts as f64),
                y(*observed)
            ));
        }
    }

    // Legend.
    svg.push_str(&format!(
        "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#1b1b1b\" stroke-width=\"1.2\"/><text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\" fill=\"#1b1b1b\" font-family=\"monospace\">modeled</text>",
        l + 6.0, t + 10.0, l + 26.0, t + 10.0, l + 31.0, t + 13.0
    ));
    svg.push_str(&format!(
        "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"3.1\" fill=\"none\" stroke=\"#8a5a2b\" stroke-width=\"1.2\"/><text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\" fill=\"#1b1b1b\" font-family=\"monospace\">observed on chain</text>",
        l + 106.0, t + 10.0, l + 114.0, t + 13.0
    ));

    // Rate path, step line, own axis.
    let rate_max = segments.iter().map(|(_, r)| *r).max().unwrap_or(1) as f64 * 1.15;
    let ry = |v: f64| rate_top + rate_h - (v / rate_max) * rate_h;
    let mut step: Vec<String> = Vec::new();
    for (index, (start, rate)) in segments.iter().enumerate() {
        let from = (*start as f64).max(t_min);
        let to = segments
            .get(index + 1)
            .map(|(next, _)| (*next as f64).min(t_max))
            .unwrap_or(t_max);
        if to < t_min || from > t_max {
            continue;
        }
        step.push(format!("{:.1},{:.1}", x(from), ry(*rate as f64)));
        step.push(format!("{:.1},{:.1}", x(to), ry(*rate as f64)));
    }
    if step.len() >= 2 {
        svg.push_str(&format!(
            "<polyline fill=\"none\" stroke=\"#5c5c5c\" stroke-width=\"1.2\" points=\"{}\"/>",
            step.join(" ")
        ));
    }
    svg.push_str(&format!(
        "<line x1=\"{l}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#d8d4cc\"/>",
        rate_top + rate_h,
        w - r,
        rate_top + rate_h
    ));
    svg.push_str(&format!(
        "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\" font-size=\"9\" fill=\"#5c5c5c\" font-family=\"monospace\">rate ppm</text>",
        l - 6.0,
        rate_top + 10.0
    ));
    for (start, rate) in &segments {
        let sx = *start as f64;
        if sx < t_min || sx > t_max {
            continue;
        }
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\" fill=\"#5c5c5c\" font-family=\"monospace\">{rate}</text>",
            x(sx) + 3.0,
            ry(*rate as f64) - 4.0
        ));
    }

    // Time axis.
    for (ts, label) in [(t_min, utc(t_min as u64)), (t_max, utc(t_max as u64))] {
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"{}\" font-size=\"9\" fill=\"#5c5c5c\" font-family=\"monospace\">{}</text>",
            x(ts),
            h - 6.0,
            if ts == t_min { "start" } else { "end" },
            escape(&label)
        ));
    }

    svg.push_str("</svg>");
    Some(svg)
}

/// mTBILL: the full round history, with the era-aware violations marked and
/// the era boundary drawn.
fn svg_mtbill(rounds: &[(u64, i128, u64, u64)], result: &Value) -> Option<String> {
    if rounds.len() < 2 {
        return None;
    }
    let violations: BTreeMap<u64, String> = result
        .get("checks")?
        .as_array()?
        .iter()
        .find(|c| c.get("id").and_then(Value::as_str) == Some("C1"))?
        .get("violations")?
        .as_array()?
        .iter()
        .filter_map(|v| {
            Some((
                v.get("round_id")?.as_u64()?,
                v.get("rule")?.as_str()?.to_string(),
            ))
        })
        .collect();

    // The era boundary, as a block, mapped to the timestamp of the first
    // round at or after it.
    let era_from_block = result
        .get("posting_eras")
        .and_then(Value::as_array)
        .and_then(|eras| eras.get(1))
        .and_then(|era| era.get("from_block"))
        .and_then(Value::as_u64);

    let (w, h) = (900.0f64, 360.0f64);
    let (l, r, t, b) = (78.0f64, 18.0f64, 30.0f64, 42.0f64);

    // A feed can open with pre-issue placeholder rounds far above the level
    // it is re-based to at launch. On a linear axis such rounds would flatten
    // the rest of the history into a straight line, so when the result labels
    // a launch re-base the axis is scaled to the history from that round on
    // and the earlier rounds are drawn off scale at the top edge with a
    // label. Every round stays counted.
    let rebase_round = result
        .get("checks")
        .and_then(Value::as_array)
        .and_then(|checks| {
            checks
                .iter()
                .find(|c| c.get("id").and_then(Value::as_str) == Some("C1"))
        })
        .and_then(|c1| c1.get("violations"))
        .and_then(Value::as_array)
        .and_then(|violations| {
            violations
                .iter()
                .find(|v| {
                    v.get("classification")
                        .and_then(Value::as_str)
                        .is_some_and(|c| c.contains("launch rebase"))
                })
                .and_then(|v| v.get("round_id"))
                .and_then(Value::as_u64)
        });

    let in_scale = |round_id: u64| rebase_round.is_none_or(|rebase| round_id >= rebase);
    let scaled: Vec<&(u64, i128, u64, u64)> = rounds
        .iter()
        .filter(|(round_id, _, _, _)| in_scale(*round_id))
        .collect();
    let off_scale: Vec<&(u64, i128, u64, u64)> = rounds
        .iter()
        .filter(|(round_id, _, _, _)| !in_scale(*round_id))
        .collect();
    if scaled.len() < 2 {
        return None;
    }

    let t_min = rounds.first().unwrap().2 as f64;
    let t_max = rounds.last().unwrap().2 as f64;
    let span = (t_max - t_min).max(1.0);
    let v_min = scaled.iter().map(|(_, a, _, _)| *a).min().unwrap_or(0) as f64;
    let v_max = scaled.iter().map(|(_, a, _, _)| *a).max().unwrap_or(1) as f64;
    let v_span = (v_max - v_min).max(1.0);

    let x = |ts: f64| l + (ts - t_min) / span * (w - l - r);
    let y = |v: f64| {
        let clamped = v.clamp(v_min, v_max);
        t + (h - t - b) - (clamped - v_min) / v_span * (h - t - b)
    };

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" xmlns=\"http://www.w3.org/2000/svg\" role=\"img\" aria-label=\"mTBILL oracle answer over the full round history, with posting-rule violations marked\">"
    ));

    for i in 0..=4 {
        let value = v_min + v_span * i as f64 / 4.0;
        let yy = y(value);
        svg.push_str(&format!(
            "<line x1=\"{l}\" y1=\"{yy:.1}\" x2=\"{:.1}\" y2=\"{yy:.1}\" stroke=\"#e6e2da\"/>",
            w - r
        ));
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\" font-size=\"9\" fill=\"#5c5c5c\" font-family=\"monospace\">{:.4}</text>",
            l - 6.0,
            yy + 3.0,
            value / 1e8
        ));
    }
    svg.push_str(&format!(
        "<text x=\"14\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"9\" fill=\"#5c5c5c\" transform=\"rotate(-90 14 {:.1})\" font-family=\"monospace\">answer, USD</text>",
        (h - b + t) / 2.0,
        (h - b + t) / 2.0
    ));

    // Era boundary: the first round posted at or after era 1's first block.
    if let Some(from_block) = era_from_block {
        if let Some((_, _, ts, _)) = rounds.iter().find(|(_, _, _, block)| *block >= from_block) {
            let bx = x(*ts as f64);
            svg.push_str(&format!(
                "<line x1=\"{bx:.1}\" y1=\"{t}\" x2=\"{bx:.1}\" y2=\"{:.1}\" stroke=\"#5c5c5c\" stroke-width=\"1\" stroke-dasharray=\"4 3\"/>",
                h - b
            ));
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\" font-size=\"9\" fill=\"#5c5c5c\" font-family=\"monospace\">era 1 begins, spacing rule added</text>",
                bx - 4.0,
                t + 11.0
            ));
        }
    }

    let path: Vec<String> = scaled
        .iter()
        .map(|(_, a, ts, _)| format!("{:.1},{:.1}", x(*ts as f64), y(*a as f64)))
        .collect();
    svg.push_str(&format!(
        "<polyline fill=\"none\" stroke=\"#1b1b1b\" stroke-width=\"1.1\" points=\"{}\"/>",
        path.join(" ")
    ));

    if !off_scale.is_empty() {
        let ids: Vec<String> = off_scale
            .iter()
            .map(|(round_id, _, _, _)| round_id.to_string())
            .collect();
        let peak = off_scale.iter().map(|(_, a, _, _)| *a).max().unwrap_or(0) as f64 / 1e8;
        for (round_id, _, ts, _) in &off_scale {
            let cx = x(*ts as f64);
            // An off-scale round can also be a violation. Marking it in the
            // accent keeps it from disappearing from the count on the page.
            let fill = if violations.contains_key(round_id) {
                "#8a5a2b"
            } else {
                "#5c5c5c"
            };
            svg.push_str(&format!(
                "<path d=\"M{:.1},{:.1} l4,7 l-8,0 z\" fill=\"{fill}\"/>",
                cx,
                t - 9.0
            ));
            if violations.contains_key(round_id) {
                svg.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"8\" fill=\"#8a5a2b\" font-family=\"monospace\">{round_id}</text>",
                    cx,
                    t - 12.0
                ));
            }
        }
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\" fill=\"#5c5c5c\" font-family=\"monospace\">rounds {}: pre-issue placeholder {:.2}, off scale</text>",
            x(off_scale.last().unwrap().2 as f64) + 8.0,
            t - 3.0,
            ids.join(" and "),
            peak
        ));
    }

    // Violation labels are staggered vertically: the early ones cluster
    // within days of each other and would otherwise overprint.
    let mut label_slot = 0usize;
    for (round_id, answer, ts, _) in rounds {
        if !in_scale(*round_id) {
            continue;
        }
        if violations.contains_key(round_id) {
            let (cx, cy) = (x(*ts as f64), y(*answer as f64));
            svg.push_str(&format!(
                "<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"4\" fill=\"none\" stroke=\"#8a5a2b\" stroke-width=\"1.4\"/>"
            ));
            let stagger = 6.0 + (label_slot % 3) as f64 * 9.0;
            label_slot += 1;
            svg.push_str(&format!(
                "<line x1=\"{cx:.1}\" y1=\"{:.1}\" x2=\"{cx:.1}\" y2=\"{:.1}\" stroke=\"#8a5a2b\" stroke-width=\".6\"/>",
                cy - 4.0,
                cy - stagger + 2.0
            ));
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-size=\"8\" fill=\"#8a5a2b\" font-family=\"monospace\">{round_id}</text>",
                cx,
                cy - stagger
            ));
        }
    }

    svg.push_str(&format!(
        "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\" fill=\"none\" stroke=\"#8a5a2b\" stroke-width=\"1.4\"/><text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\" fill=\"#1b1b1b\" font-family=\"monospace\">round posted outside the rules in force</text>",
        l + 10.0,
        h - b - 12.0,
        l + 19.0,
        h - b - 9.0
    ));
    for (ts, anchor) in [(t_min, "start"), (t_max, "end")] {
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"{anchor}\" font-size=\"9\" fill=\"#5c5c5c\" font-family=\"monospace\">{}</text>",
            x(ts),
            h - b + 25.0,
            escape(&utc(ts as u64))
        ));
    }
    svg.push_str("</svg>");
    Some(svg)
}

// ---------------------------------------------------------------------------
// Reading a bundle
// ---------------------------------------------------------------------------

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// The mTBILL round series, located through the manifest rather than assumed:
/// the manifest names the raw file that holds the AnswerUpdated response, and
/// that response carries answer, round id and timestamp in its topics.
fn mtbill_rounds(run: &Run) -> Vec<(u64, i128, u64, u64)> {
    let Some(manifest) = &run.manifest else {
        return Vec::new();
    };
    let Some(entries) = manifest.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    let Some(entry) = entries.iter().find(|entry| {
        entry
            .get("label")
            .and_then(Value::as_str)
            .is_some_and(|label| label.contains("AnswerUpdated"))
    }) else {
        return Vec::new();
    };
    let Some(file) = entry.get("file").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(body) = read_json(&run.dir.join(file)) else {
        return Vec::new();
    };
    let Some(rows) = body.get("result").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut out: Vec<(u64, i128, u64, u64)> = rows
        .iter()
        .filter_map(|row| {
            let topics = row.get("topics")?.as_array()?;
            let answer =
                i128::from_str_radix(topics.get(1)?.as_str()?.trim_start_matches("0x"), 16).ok()?;
            let round_id =
                u64::from_str_radix(topics.get(2)?.as_str()?.trim_start_matches("0x"), 16).ok()?;
            let timestamp =
                u64::from_str_radix(topics.get(3)?.as_str()?.trim_start_matches("0x"), 16).ok()?;
            let block = u64::from_str_radix(
                row.get("blockNumber")?.as_str()?.trim_start_matches("0x"),
                16,
            )
            .ok()?;
            Some((round_id, answer, timestamp, block))
        })
        .collect();
    out.sort_by_key(|(round_id, _, _, _)| *round_id);
    out
}

/// The verdict line, whichever field the target uses for it.
fn verdict_of(result: &Value) -> String {
    result
        .get("verdict")
        .and_then(Value::as_str)
        .or_else(|| result.get("consistency").and_then(Value::as_str))
        .unwrap_or("UNKNOWN")
        .to_string()
}

/// The check class in plain words, per target.
fn check_class_words(result: &Value) -> &'static str {
    match result.get("target").and_then(Value::as_str) {
        Some("svzchf") => "Full recomputation, zero tolerance. Every compared value is recomputed from public inputs and must match the chain to the wei.",
        Some("mtbill") => "Consistency bundle. The NAV itself is INPUT_GAP: the underlying portfolio is not observable, so it is not recomputed here. Everything below checks the issuer's own contractual and on-chain rules against itself.",
        Some("midas") => "Posting-path replay. No Midas NAV is recomputed (INPUT_GAP on every feed). Every posted round is attributed to the function that posted it, and the guard state in force at the previous block is read from an archive node. A post that took the documented path without the on-chain deviation check and moved more than the bound in force is reported; nothing here says a posted value was wrong.",
        _ => "Unknown target.",
    }
}

/// The headline number for the index row.
fn headline(result: &Value) -> String {
    match result.get("target").and_then(Value::as_str) {
        Some("svzchf") => {
            let fields = result
                .get("comparison")
                .and_then(|c| c.get("fields"))
                .and_then(Value::as_array);
            match fields {
                Some(fields) => {
                    let nonzero = fields
                        .iter()
                        .filter(|f| f.get("equal").and_then(Value::as_bool) == Some(false))
                        .count();
                    if nonzero == 0 {
                        format!(
                            "{} of {} fields exact, residual 0",
                            fields.len(),
                            fields.len()
                        )
                    } else {
                        format!("{nonzero} of {} fields deviate", fields.len())
                    }
                }
                None => "no comparison".to_string(),
            }
        }
        Some("midas") => str_at(result, &["summary", "headline"])
            .unwrap_or("")
            .to_string(),
        Some("mtbill") => {
            let checks = result.get("checks").and_then(Value::as_array);
            match checks {
                Some(checks) => {
                    let violations: usize = checks
                        .iter()
                        .map(|c| {
                            c.get("violations")
                                .and_then(Value::as_array)
                                .map(|v| v.len())
                                .unwrap_or(0)
                        })
                        .sum();
                    let failing = result
                        .get("failing_checks")
                        .and_then(Value::as_array)
                        .map(|f| f.len())
                        .unwrap_or(0);
                    format!("{violations} violation(s) across {failing} failing check(s)")
                }
                None => "no checks".to_string(),
            }
        }
        _ => String::new(),
    }
}

/// The exact command that reproduces the run.
fn reproduce_command(result: &Value) -> String {
    let baseline = u64_at(result, &["window", "baseline_block"]);
    let block = u64_at(result, &["window", "block"]);
    match (
        result.get("target").and_then(Value::as_str),
        baseline,
        block,
    ) {
        (Some(target), Some(baseline), Some(block)) => {
            format!("crossfoot run {target} --baseline-block {baseline} --block {block}")
        }
        (Some("midas"), None, Some(block)) => format!("crossfoot run midas --block {block}"),
        _ => "unknown".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn slug(name: &str) -> String {
    crate::util::slug(name)
}

fn provenance(run: &Run) -> String {
    let result = &run.result;
    let entry_count = run
        .manifest
        .as_ref()
        .and_then(|m| m.get("entry_count"))
        .and_then(Value::as_u64);
    let meta = read_json(&run.dir.join("meta.json"));
    let get_meta = |key: &str| -> Option<String> {
        let meta = meta.as_ref()?;
        match meta.get(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Array(a) => Some(
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<&str>>()
                    .join(", "),
            ),
            other => Some(other.to_string()),
        }
    };

    let mut rows: Vec<(String, String)> = vec![
        ("bundle path".into(), format!("bundles/{}", run.name)),
        (
            "bundle root hash".into(),
            run.root_hash.clone().unwrap_or("not sealed".into()),
        ),
        ("result.json sha256".into(), run.result_sha256.clone()),
        (
            "raw responses in this bundle".into(),
            entry_count.map(|c| c.to_string()).unwrap_or("n/a".into()),
        ),
        (
            "cache hits this run".into(),
            get_meta("cache_hits_this_run").unwrap_or("n/a".into()),
        ),
        (
            "network calls this run".into(),
            get_meta("network_calls_this_run").unwrap_or("n/a".into()),
        ),
        (
            "rpc endpoints".into(),
            get_meta("endpoints_configured").unwrap_or("n/a".into()),
        ),
        (
            "log endpoints".into(),
            get_meta("log_endpoints_configured").unwrap_or("n/a".into()),
        ),
        (
            "repo git commit".into(),
            meta.as_ref()
                .and_then(|m| str_at(m, &["repo_git", "commit"]))
                .unwrap_or("n/a")
                .to_string(),
        ),
        (
            "repo git dirty at run time".into(),
            meta.as_ref()
                .and_then(|m| m.get("repo_git"))
                .and_then(|g| g.get("dirty"))
                .map(|d| d.to_string())
                .unwrap_or("n/a".into()),
        ),
        ("reproduce with".into(), reproduce_command(result)),
    ];

    if let Some(commit) = str_at(result, &["contract_sources", "commit"]) {
        let repo = str_at(result, &["contract_sources", "repo"]).unwrap_or("");
        rows.insert(
            rows.len() - 1,
            ("contract sources".into(), format!("{repo} at {commit}")),
        );
    }
    // Timings are a property of the run, not of the result, so they live in
    // meta.json (spec 01 R8).
    if let Some(started) = get_meta("run_started_utc") {
        rows.insert(2, ("run started (UTC)".into(), started));
    }

    let mut html = String::from("<h2>Provenance</h2>\n<dl class=\"kv\">\n");
    for (key, value) in rows {
        html.push_str(&format!(
            "<dt>{}</dt><dd>{}</dd>\n",
            escape(&key),
            escape(&value)
        ));
    }
    html.push_str("</dl>\n");
    html
}

fn window_block(result: &Value) -> String {
    let w = result.get("window");
    let b0 = w
        .and_then(|w| w.get("baseline_block"))
        .and_then(Value::as_u64);
    let b1 = w.and_then(|w| w.get("block")).and_then(Value::as_u64);
    let t0 = w
        .and_then(|w| w.get("baseline_timestamp_unix"))
        .and_then(Value::as_u64);
    let t1 = w
        .and_then(|w| w.get("block_timestamp_unix"))
        .and_then(Value::as_u64);
    if b0.is_none() && t0.is_none() {
        return format!(
            "block {}<br><span class=\"note\">{}, history from block 0</span>",
            b1.map(|v| v.to_string()).unwrap_or("?".into()),
            t1.map(utc).unwrap_or("?".into())
        );
    }
    format!(
        "block {} to {}<br><span class=\"note\">{} to {}</span>",
        b0.map(|v| v.to_string()).unwrap_or("?".into()),
        b1.map(|v| v.to_string()).unwrap_or("?".into()),
        t0.map(utc).unwrap_or("?".into()),
        t1.map(utc).unwrap_or("?".into())
    )
}

fn svzchf_body(run: &Run) -> String {
    let result = &run.result;
    let mut html = String::new();

    html.push_str("<h2>Comparison</h2>\n");
    html.push_str("<p class=\"note\">Modeled is recomputed from the log-derived rate path and the account's own flow history. Observed is read from the chain at the pinned block. Tolerance is zero.</p>\n");
    html.push_str(
        "<table>\n<tr><th>field</th><th>modeled</th><th>observed</th><th>residual</th><th>equal</th></tr>\n",
    );
    if let Some(fields) = result
        .get("comparison")
        .and_then(|c| c.get("fields"))
        .and_then(Value::as_array)
    {
        for field in fields {
            let equal = field.get("equal").and_then(Value::as_bool).unwrap_or(false);
            let name = field.get("field").and_then(Value::as_str).unwrap_or("");
            let modeled = field.get("modeled").and_then(Value::as_str).unwrap_or("");
            let observed = field.get("observed").and_then(Value::as_str).unwrap_or("");
            // Everything except the tick counter is an 18 decimal amount. The
            // raw integer stays on the page; the readable form is a second
            // line under it, never a replacement.
            let readable = |value: &str| -> String {
                if name.contains("ticks") {
                    String::new()
                } else {
                    format!(
                        "<br><span class=\"note\">{}</span>",
                        escape(&wei(value, 18))
                    )
                }
            };
            html.push_str(&format!(
                "<tr><td>{}</td><td class=\"num\">{}{}</td><td class=\"num\">{}{}</td><td class=\"num{}\">{}</td><td class=\"mono\">{}</td></tr>\n",
                escape(name),
                escape(modeled),
                readable(modeled),
                escape(observed),
                readable(observed),
                if equal { "" } else { " flag" },
                escape(field.get("residual").and_then(Value::as_str).unwrap_or("")),
                equal
            ));
        }
    }
    html.push_str("</table>\n");

    if let Some(svg) = svg_svzchf(result) {
        let observed = result
            .get("replay_steps")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .filter(|s| s.get("observed_interest").and_then(Value::as_str).is_some())
                    .count()
            })
            .unwrap_or(0);
        html.push_str("<h2>Recognised interest per recognition event</h2>\n<figure>\n");
        html.push_str(&svg);
        html.push_str(&format!(
            "\n<figcaption>Modeled recognised interest as a line, the {observed} on-chain InterestCollected amounts as circles. Every circle sits on the line: the model reproduces each recognition to the wei, not only the endpoints. The administered rate path is the step line beneath.</figcaption>\n</figure>\n"
        ));
    }

    html.push_str("<h2>Two independent implementations</h2>\n");
    if let Some(cross) = result.get("actus_cross_check") {
        html.push_str("<div class=\"panel\">\n");
        html.push_str(&format!(
            "<p>The integer replay of the deployed state machine and the ACTUS engine path agree at {} recognition points plus the horizon: <span class=\"mono\">{}</span>.</p>\n",
            cross.get("recognition_points_compared").and_then(Value::as_u64).unwrap_or(0),
            if cross.get("agree").and_then(Value::as_bool) == Some(true) { "agree" } else { "DISAGREE" }
        ));
        if let Some(margin) = cross
            .get("smallest_distance_to_the_flooring_boundary_wei")
            .and_then(Value::as_str)
        {
            html.push_str(&format!(
                "<p class=\"note\">Agreement is by measured margin, not by construction: the engine carries exact decimals, so the two paths agree as long as the exact value is not within the decimal error of an integer wei. Smallest distance to a flooring boundary observed on this run: {} wei, against an error budget near 1e-9 wei.</p>\n",
                escape(margin)
            ));
        }
        html.push_str("</div>\n");
    }

    html
}

fn mtbill_body(run: &Run) -> String {
    let result = &run.result;
    let mut html = String::new();

    html.push_str("<div class=\"panel\">\n<p><strong>nav_recomputation: INPUT_GAP</strong> (underlying portfolio not observable)</p>\n");
    if let Some(reason) = result
        .get("nav_recomputation_reason")
        .and_then(Value::as_str)
    {
        html.push_str(&format!("<p class=\"note\">{}</p>\n", escape(reason)));
    }
    html.push_str("</div>\n");

    html.push_str("<h2>Checks</h2>\n<table>\n<tr><th>check</th><th>name</th><th>check class</th><th>verdict</th><th>summary</th></tr>\n");
    if let Some(checks) = result.get("checks").and_then(Value::as_array) {
        for check in checks {
            let id = check.get("id").and_then(Value::as_str).unwrap_or("");
            let verdict = check
                .get("verdict")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_uppercase();
            let flag = matches!(
                verdict.as_str(),
                "OBSERVED_DEVIATION" | "INPUT_GAP" | "MODEL_INCONSISTENT" | "INSUFFICIENT_WINDOW"
            );
            let mut summary = check
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id == "C8" {
                summary.push_str(" [not an independent source: DefiLlama may take this price from the same oracle]");
            }
            html.push_str(&format!(
                "<tr><td class=\"mono\">{}</td><td>{}</td><td class=\"note\">{}</td><td class=\"verdict{}\">{}</td><td>{}</td></tr>\n",
                escape(id),
                escape(check.get("name").and_then(Value::as_str).unwrap_or("")),
                escape(check.get("check_class").and_then(Value::as_str).unwrap_or("")),
                if flag { " flag" } else { "" },
                escape(&verdict),
                escape(&summary)
            ));
        }
    }
    html.push_str("</table>\n");

    let rounds = mtbill_rounds(run);
    if let Some(svg) = svg_mtbill(&rounds, result) {
        html.push_str("<h2>Round history and posting-rule violations</h2>\n<figure>\n");
        html.push_str(&svg);
        html.push_str(&format!(
            "\n<figcaption>All {} posted rounds are counted. The y axis is scaled to the post-rebase history, so any pre-issue placeholder rounds sit off scale at the top edge and are marked there. Circled rounds could not have been posted through setRoundDataSafe under the rules the implementation in force at that block actually enforced. The dashed line is the upgrade that added the one-hour spacing requirement; that rule is not applied to rounds posted before it.</figcaption>\n</figure>\n",
            rounds.len()
        ));
    }

    // Violations with era and attribution.
    let attribution: BTreeMap<u64, &Value> = result
        .get("attribution")
        .and_then(|a| a.get("rounds"))
        .and_then(Value::as_array)
        .map(|rounds| {
            rounds
                .iter()
                .filter_map(|r| Some((r.get("round_id")?.as_u64()?, r)))
                .collect()
        })
        .unwrap_or_default();

    if let Some(c1) = result
        .get("checks")
        .and_then(Value::as_array)
        .and_then(|checks| {
            checks
                .iter()
                .find(|c| c.get("id").and_then(Value::as_str) == Some("C1"))
        })
    {
        if let Some(violations) = c1.get("violations").and_then(Value::as_array) {
            html.push_str("<h3>Violations</h3>\n<table>\n<tr><th>round</th><th>when (UTC)</th><th>era</th><th>posted via</th><th>from</th><th>rule</th></tr>\n");
            for violation in violations {
                let round_id = violation.get("round_id").and_then(Value::as_u64);
                let entry = round_id.and_then(|id| attribution.get(&id));
                let classification = violation
                    .get("classification")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let rule = violation.get("rule").and_then(Value::as_str).unwrap_or("");
                let label = if classification.contains("launch rebase") {
                    format!("{rule} <span class=\"note\">[launch rebase, not a manipulation signal]</span>")
                } else {
                    escape(rule)
                };
                html.push_str(&format!(
                    "<tr><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td>{}</td></tr>\n",
                    round_id.map(|v| v.to_string()).unwrap_or_default(),
                    violation
                        .get("updated_at")
                        .and_then(Value::as_u64)
                        .map(utc)
                        .unwrap_or_default(),
                    violation
                        .get("era")
                        .map(|e| e.to_string())
                        .unwrap_or("?".into()),
                    escape(
                        entry
                            .and_then(|e| e.get("function"))
                            .and_then(Value::as_str)
                            .unwrap_or("not attributed")
                    ),
                    escape(
                        entry
                            .and_then(|e| e.get("from"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    ),
                    label
                ));
            }
            html.push_str("</table>\n");
        }
    }

    if let Some(answer) = str_at(result, &["c1_c2_cross_reference", "second_answer"]) {
        html.push_str(&format!(
            "<div class=\"panel\"><p class=\"note\"><strong>Were the rules the same across the upgrades?</strong> {}</p></div>\n",
            escape(answer)
        ));
    }
    if let Some(answer) = str_at(result, &["c1_c2_cross_reference", "answer"]) {
        html.push_str(&format!(
            "<div class=\"panel\"><p class=\"note\"><strong>Could a parameter change explain the violations?</strong> {}</p></div>\n",
            escape(answer)
        ));
    }

    html
}

// ---------------------------------------------------------------------------
// Midas family page
// ---------------------------------------------------------------------------

/// The per-feed timeline, read from the timeline file the run wrote, never
/// from the result: the file is what the consumer agent reads too.
fn midas_timeline(run: &Run, feed: &Value) -> Option<Value> {
    let file = feed.get("timeline_file")?.as_str()?;
    read_json(&run.dir.join(file))
}

/// Draws one feed's posting history from its timeline file: rounds that went
/// through the checked path in the plain colour, every post that took the
/// unchecked path in the accent colour with its transaction hash as text,
/// and in a second panel the deviation in force of every checked post
/// against the bound in force as a stepped line.
fn svg_midas_timeline(timeline: &Value, feed_name: &str) -> Option<String> {
    let rounds = timeline.get("rounds")?.as_array()?;
    if rounds.is_empty() {
        return None;
    }
    let decimals = timeline
        .get("decimals")
        .and_then(Value::as_u64)
        .unwrap_or(8) as i32;
    // (round id, answer, path, transaction hash, finding, deviation, bound)
    type Mark<'a> = (
        u64,
        f64,
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<f64>,
        Option<f64>,
    );
    let answers: Vec<Mark> = rounds
        .iter()
        .filter_map(|r| {
            let answer: f64 =
                r.get("answer")?.as_str()?.parse::<i128>().ok()? as f64 / 10f64.powi(decimals);
            let percent = |key: &str| -> Option<f64> {
                r.get(key)?
                    .as_str()?
                    .parse::<i128>()
                    .ok()
                    .map(|v| v as f64 / 10f64.powi(decimals))
            };
            Some((
                r.get("round_id")?.as_u64()?,
                answer,
                r.get("path")?.as_str()?,
                r.get("transaction_hash")?.as_str()?,
                r.get("finding").and_then(Value::as_str),
                percent("deviation_in_force"),
                percent("bound_in_force"),
            ))
        })
        .collect();
    if answers.is_empty() {
        return None;
    }

    let width = 900.0;
    let left = 70.0;
    let right = 20.0;
    let plot_width = width - left - right;
    let top_height = 220.0;
    let gap = 40.0;
    let bottom_height = 150.0;
    let height = 30.0 + top_height + gap + bottom_height + 40.0;
    let n = answers.len() as f64;
    let x_of = |index: usize| left + (index as f64 + 0.5) / n * plot_width;

    // Top panel: answers. Values more than 100x away from the median are
    // placeholders or scale resets and are drawn off scale at the edge so
    // they do not flatten the axis.
    let mut sorted: Vec<f64> = answers.iter().map(|a| a.1).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let in_scale: Vec<f64> = sorted
        .iter()
        .copied()
        .filter(|v| *v > median / 100.0 && *v < median * 100.0)
        .collect();
    let (min_v, max_v) = (
        in_scale.first().copied().unwrap_or(median),
        in_scale.last().copied().unwrap_or(median),
    );
    let span = if max_v > min_v {
        max_v - min_v
    } else {
        median.abs().max(1.0) * 0.01
    };
    let top_y0 = 30.0;
    let y_of = |value: f64| {
        let clamped = value.max(min_v - span * 0.5).min(max_v + span * 0.5);
        top_y0 + top_height - (clamped - (min_v - span * 0.05)) / (span * 1.1) * top_height
    };

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width} {height}\" width=\"100%\" role=\"img\" aria-label=\"{}\">\n",
        escape(&format!("{feed_name} posting history"))
    ));
    svg.push_str(&format!(
        "<text x=\"{left}\" y=\"18\" font-size=\"12\" fill=\"var(--muted)\">{} answers, one mark per round, in round order; accent marks took the path without the on-chain deviation check</text>\n",
        escape(feed_name)
    ));
    svg.push_str(&format!(
        "<line x1=\"{left}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"var(--rule)\"/>\n",
        top_y0 + top_height,
        width - right,
        top_y0 + top_height
    ));
    for (label, value) in [("high", max_v), ("low", min_v)] {
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{:.1}\" font-size=\"10\" fill=\"var(--muted)\" text-anchor=\"end\">{label} {:.6}</text>\n",
            left - 6.0,
            y_of(value) + 3.0,
            value
        ));
    }
    // The checked path as a polyline, so the eye follows the series.
    let mut points = String::new();
    for (index, a) in answers.iter().enumerate() {
        points.push_str(&format!("{:.1},{:.1} ", x_of(index), y_of(a.1)));
    }
    svg.push_str(&format!(
        "<polyline points=\"{}\" fill=\"none\" stroke=\"var(--rule)\" stroke-width=\"1\"/>\n",
        points.trim_end()
    ));
    for (index, a) in answers.iter().enumerate() {
        let unchecked = a.2 == "raw" || a.2 == "raw3";
        let colour = if unchecked {
            "var(--accent)"
        } else {
            "var(--ink)"
        };
        let radius = if unchecked { 4.5 } else { 2.0 };
        svg.push_str(&format!(
            "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{radius}\" fill=\"{colour}\"><title>round {} {} {}{}</title></circle>\n",
            x_of(index),
            y_of(a.1),
            a.0,
            a.2,
            a.1,
            a.4.map(|f| format!(" {f}")).unwrap_or_default()
        ));
        if unchecked {
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\" fill=\"var(--accent)\" text-anchor=\"middle\">r{} {}</text>\n",
                x_of(index),
                y_of(a.1) - 8.0,
                a.0,
                escape(a.3)
            ));
        }
    }

    // Bottom panel: deviation in force against the bound in force.
    let bottom_y0 = top_y0 + top_height + gap;
    let bound_samples: Vec<(u64, f64)> = timeline
        .get("bound_samples")
        .and_then(Value::as_array)
        .map(|samples| {
            samples
                .iter()
                .filter_map(|s| {
                    Some((
                        s.get("block")?.as_u64()?,
                        s.get("bound")?.as_str()?.parse::<i128>().ok()? as f64
                            / 10f64.powi(decimals),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut max_percent = bound_samples.iter().map(|(_, b)| *b).fold(0.0f64, f64::max);
    for a in &answers {
        if let Some(dev) = a.5 {
            // Scale resets are off the chart by construction; cap the axis.
            max_percent = max_percent.max(dev.min(25.0));
        }
    }
    let max_percent = if max_percent > 0.0 {
        max_percent * 1.15
    } else {
        1.0
    };
    let py_of = |percent: f64| {
        bottom_y0 + bottom_height - percent.min(max_percent) / max_percent * bottom_height
    };
    svg.push_str(&format!(
        "<text x=\"{left}\" y=\"{:.1}\" font-size=\"12\" fill=\"var(--muted)\">deviation in force of every checked post (percent) against the bound in force (stepped line)</text>\n",
        bottom_y0 - 10.0
    ));
    svg.push_str(&format!(
        "<line x1=\"{left}\" y1=\"{:.1}\" x2=\"{}\" y2=\"{:.1}\" stroke=\"var(--rule)\"/>\n",
        bottom_y0 + bottom_height,
        width - right,
        bottom_y0 + bottom_height
    ));
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{:.1}\" font-size=\"10\" fill=\"var(--muted)\" text-anchor=\"end\">{:.2}%</text>\n",
        left - 6.0,
        py_of(max_percent) + 3.0,
        max_percent
    ));
    // Bound in force: the bound at each round is the latest sample at or
    // before the round's block, drawn as a step function over round index.
    let blocks: Vec<u64> = rounds
        .iter()
        .filter_map(|r| r.get("block").and_then(Value::as_u64))
        .collect();
    if !bound_samples.is_empty() && blocks.len() == answers.len() {
        let mut step = String::new();
        for (index, block) in blocks.iter().enumerate() {
            let bound = bound_samples
                .iter()
                .rev()
                .find(|(b, _)| *b <= *block)
                .or(bound_samples.first())
                .map(|(_, v)| *v)
                .unwrap_or(0.0);
            let x0 = left + index as f64 / n * plot_width;
            let x1 = left + (index as f64 + 1.0) / n * plot_width;
            step.push_str(&format!(
                "{:.1},{:.1} {:.1},{:.1} ",
                x0,
                py_of(bound),
                x1,
                py_of(bound)
            ));
        }
        svg.push_str(&format!(
            "<polyline points=\"{}\" fill=\"none\" stroke=\"var(--ink)\" stroke-width=\"1.5\" stroke-dasharray=\"4 3\"><title>bound in force</title></polyline>\n",
            step.trim_end()
        ));
    }
    for (index, a) in answers.iter().enumerate() {
        let Some(dev) = a.5 else { continue };
        let unchecked = a.2 == "raw" || a.2 == "raw3";
        let colour = if unchecked {
            "var(--accent)"
        } else {
            "var(--ink)"
        };
        let over = a.6.is_some_and(|bound| dev > bound);
        svg.push_str(&format!(
            "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{colour}\" stroke-width=\"{}\"><title>round {} deviation {:.5}% bound {}</title></line>\n",
            x_of(index),
            bottom_y0 + bottom_height,
            x_of(index),
            py_of(dev),
            if over { 3.0 } else { 1.5 },
            a.0,
            dev,
            a.6.map(|b| format!("{b:.3}%")).unwrap_or("n/a".into())
        ));
    }
    svg.push_str("</svg>\n");
    Some(svg)
}

fn midas_body(run: &Run) -> String {
    let result = &run.result;
    let mut html = String::new();

    html.push_str("<div class=\"panel\">\n<p><strong>nav_recomputation: INPUT_GAP</strong> (no Midas NAV is recomputed)</p>\n");
    if let Some(reason) = result
        .get("nav_recomputation_reason")
        .and_then(Value::as_str)
    {
        html.push_str(&format!("<p class=\"note\">{}</p>\n", escape(reason)));
    }
    if let Some(line) = str_at(result, &["summary", "headline"]) {
        html.push_str(&format!("<p class=\"verdict\">{}</p>\n", escape(line)));
    }
    html.push_str("</div>\n");

    // Family summary.
    if let Some(summary) = result.get("family_summary") {
        html.push_str("<h2>Family summary</h2>\n<dl class=\"kv\">\n");
        let counts = |key: &str| -> String {
            summary
                .get(key)
                .map(|v| match v {
                    Value::Object(map) => map
                        .iter()
                        .map(|(k, v)| format!("{k} {v}"))
                        .collect::<Vec<String>>()
                        .join(", "),
                    other => other.to_string(),
                })
                .unwrap_or("n/a".into())
        };
        for (label, key) in [
            ("feeds configured", "feeds_configured"),
            ("feeds replayed", "feeds_replayed"),
            ("feeds derived (no guard to replay)", "feeds_derived"),
            ("feeds unreadable", "feeds_unreadable"),
            ("rounds", "rounds_total"),
            ("successful posts, external", "posts_external"),
            ("successful posts, Safe-routed", "posts_internal"),
            ("failed setters", "failed_setters"),
            (
                "unchecked posts over the bound in force, external",
                "bypass_posts_external",
            ),
            (
                "unchecked posts over the bound in force, Safe-routed",
                "bypass_posts_internal",
            ),
            (
                "unchecked posts over the bound in force, total",
                "bypass_posts_total",
            ),
            ("feeds with at least one", "feeds_with_bypass"),
            ("classification", "bypass_classification"),
            ("recent subset", "recent"),
            ("liveness", "liveness"),
            ("bound changes", "bound_changes"),
            ("attribution gaps", "attribution_gaps"),
        ] {
            html.push_str(&format!(
                "<dt>{}</dt><dd>{}</dd>\n",
                escape(label),
                escape(&counts(key))
            ));
        }
        html.push_str("</dl>\n");
    }

    // One row per feed.
    let feeds = result
        .get("feeds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    html.push_str("<h2>Feeds</h2>\n<table>\n<tr><th>feed</th><th>address</th><th>kind</th><th>posts (safe / safe3 / raw / raw3 / failed)</th><th>over bound</th><th>posting path</th><th>liveness</th><th>verdict</th><th>action</th></tr>\n");
    for feed in &feeds {
        let s = |key: &str| {
            feed.get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let posts = feed.get("posts");
        let count = |key: &str| {
            posts
                .and_then(|p| p.get(key))
                .and_then(Value::as_u64)
                .map(|v| v.to_string())
                .unwrap_or("-".into())
        };
        let verdict = s("verdict");
        let flag = verdict != "CONSISTENT";
        html.push_str(&format!(
            "<tr><td>{}.{}</td><td class=\"mono\">{}</td><td>{}</td><td class=\"mono\">{} / {} / {} / {} / {}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"verdict{}\">{}</td><td>{}</td></tr>\n",
            escape(&s("product")),
            escape(&s("key")),
            escape(&s("address")),
            escape(&s("kind")),
            count("safe"),
            count("safe3"),
            count("raw"),
            count("raw3"),
            count("failed"),
            feed.get("bypass_posts").and_then(Value::as_u64).unwrap_or(0),
            escape(&s("posting_path")),
            escape(&s("liveness")),
            if flag { " flag" } else { "" },
            escape(&verdict),
            escape(&s("consumer_action"))
        ));
    }
    html.push_str("</table>\n");

    // Timelines and findings for every feed with at least one post that took
    // the path without the on-chain check and exceeded the bound in force.
    for feed in &feeds {
        let bypasses = feed
            .get("bypass_posts")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if bypasses == 0 {
            continue;
        }
        let name = format!(
            "{}.{}",
            feed.get("product").and_then(Value::as_str).unwrap_or(""),
            feed.get("key").and_then(Value::as_str).unwrap_or("")
        );
        html.push_str(&format!("<h2>{}</h2>\n", escape(&name)));
        if let Some(timeline) = midas_timeline(run, feed) {
            if let Some(svg) = svg_midas_timeline(&timeline, &name) {
                html.push_str("<div class=\"figure\">\n");
                html.push_str(&svg);
                html.push_str("</div>\n");
                html.push_str(&format!(
                    "<p class=\"note\">Drawn from {} in the bundle, {} rounds.</p>\n",
                    escape(
                        feed.get("timeline_file")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    ),
                    timeline
                        .get("rounds")
                        .and_then(Value::as_array)
                        .map(|r| r.len())
                        .unwrap_or(0)
                ));
            }
        }
        html.push_str("<table>\n<tr><th>finding</th><th>round</th><th>block</th><th>time (UTC)</th><th>path</th><th>value</th><th>last answer at block minus one</th><th>deviation in force</th><th>bound in force</th><th>classification</th><th>transaction</th></tr>\n");
        if let Some(findings) = feed.get("findings").and_then(Value::as_array) {
            for finding in findings {
                let s = |key: &str| {
                    finding
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                let kind = s("kind");
                if !matches!(
                    kind.as_str(),
                    "GUARD_BYPASS" | "UNGUARDED_POST" | "GUARD_INCONSISTENT" | "BOUND_CHANGED"
                ) {
                    continue;
                }
                let timestamp = finding
                    .get("timestamp_unix")
                    .and_then(Value::as_u64)
                    .map(utc)
                    .unwrap_or_default();
                let (value, old_new) = if kind == "BOUND_CHANGED" {
                    (
                        String::new(),
                        format!(
                            "{} to {}",
                            finding
                                .get("old")
                                .and_then(|o| o.get("bound_percent"))
                                .and_then(Value::as_str)
                                .unwrap_or("?"),
                            finding
                                .get("new")
                                .and_then(|o| o.get("bound_percent"))
                                .and_then(Value::as_str)
                                .unwrap_or("?")
                        ),
                    )
                } else {
                    (s("value"), String::new())
                };
                html.push_str(&format!(
                    "<tr><td class=\"mono\">{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td>{}</td><td class=\"mono\">{}</td></tr>\n",
                    escape(&kind),
                    finding.get("round_id").and_then(Value::as_u64).map(|r| r.to_string()).unwrap_or_default(),
                    finding.get("block").and_then(Value::as_u64).map(|b| b.to_string()).unwrap_or_default(),
                    escape(&timestamp),
                    escape(&s("path")),
                    escape(&value),
                    escape(&s("last_answer_at_block_minus_one")),
                    escape(&if kind == "BOUND_CHANGED" { old_new.clone() } else { format!("{}%", s("deviation_percent")) }),
                    escape(&if kind == "BOUND_CHANGED" { String::new() } else { format!("{}%", s("bound_percent")) }),
                    escape(&s("classification")),
                    escape(&s("transaction_hash"))
                ));
            }
        }
        html.push_str("</table>\n");
    }

    if let Some(method) = result.get("method").and_then(Value::as_object) {
        html.push_str("<h2>Method</h2>\n<dl class=\"kv\">\n");
        for (key, value) in method {
            html.push_str(&format!(
                "<dt>{}</dt><dd>{}</dd>\n",
                escape(key),
                escape(value.as_str().unwrap_or(""))
            ));
        }
        html.push_str("</dl>\n");
    }
    html
}

// ---------------------------------------------------------------------------
// Machine readable export: site/data/feeds.json and site/data/timelines/
// ---------------------------------------------------------------------------

/// One feeds.json row. The index row and this row read `summary` for the
/// verdict, the consumer action and the headline whenever the result carries
/// one, and fall back to the target specific fields otherwise.
fn feed_row(run: &Run, feed: Option<&Value>) -> Value {
    let result = &run.result;
    let summary = result.get("summary");
    let target = result.get("target").and_then(Value::as_str).unwrap_or("");
    let s = |value: Option<&Value>, key: &str| -> Option<String> {
        value?.get(key)?.as_str().map(|s| s.to_string())
    };
    let (
        address,
        product,
        key,
        kind,
        verdict,
        posting_path,
        liveness,
        action,
        nav,
        headline_text,
        timeline,
    ) = match (target, feed) {
        ("midas", Some(feed)) => (
            s(Some(feed), "address"),
            s(Some(feed), "product"),
            s(Some(feed), "key"),
            s(Some(feed), "kind"),
            s(Some(feed), "verdict"),
            s(Some(feed), "posting_path"),
            s(Some(feed), "liveness"),
            s(Some(feed), "consumer_action"),
            s(Some(feed), "nav_recomputation"),
            Some(format!(
                "{} unchecked post(s) over the bound in force, posting path {}, liveness {}",
                feed.get("bypass_posts")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                s(Some(feed), "posting_path").unwrap_or("n/a".into()),
                s(Some(feed), "liveness").unwrap_or("n/a".into())
            )),
            s(Some(feed), "timeline_file")
                .map(|file| format!("data/timelines/{}", file.trim_start_matches("timelines/"))),
        ),
        ("svzchf", _) => (
            Some(crate::svzchf::VAULT.to_string()),
            Some("svZCHF".to_string()),
            Some("vault".to_string()),
            Some("recomputable".to_string()),
            s(summary, "verdict").or_else(|| Some(verdict_of(result))),
            None,
            None,
            s(summary, "consumer_action").or_else(|| {
                Some(
                    if verdict_of(result) == "MODEL_MATCH" {
                        "ALLOW"
                    } else {
                        "REVIEW"
                    }
                    .to_string(),
                )
            }),
            s(summary, "nav_recomputation").or(Some("FULL".to_string())),
            s(summary, "headline").or_else(|| Some(headline(result))),
            None,
        ),
        _ => (
            Some(crate::mtbill::ORACLE.to_string()),
            Some("mTBILL".to_string()),
            Some("customFeed".to_string()),
            Some("bounded".to_string()),
            s(summary, "verdict").or_else(|| Some(verdict_of(result))),
            None,
            None,
            s(summary, "consumer_action").or_else(|| {
                Some(
                    if verdict_of(result) == "CONSISTENT" {
                        "ALLOW"
                    } else {
                        "REVIEW"
                    }
                    .to_string(),
                )
            }),
            s(summary, "nav_recomputation").or(Some("INPUT_GAP".to_string())),
            s(summary, "headline").or_else(|| Some(headline(result))),
            None,
        ),
    };
    let family = s(summary, "family").unwrap_or_else(|| {
        match target {
            "svzchf" => "recomputable-accrual",
            _ => "guarded-setter",
        }
        .to_string()
    });
    serde_json::json!({
        "address": address,
        "target": target,
        "product": product,
        "key": key,
        "kind": kind,
        "family": family,
        "verdict": verdict,
        "posting_path": posting_path,
        "liveness": liveness,
        "consumer_action": action,
        "nav_recomputation": nav,
        "headline": headline_text,
        "bundle": run.name,
        "bundle_root": run.root_hash,
        "result_path": format!("bundles/{}/result.json", run.name),
        "result_sha256": run.result_sha256,
        "block": u64_at(result, &["window", "block"]),
        "baseline_block": u64_at(result, &["window", "baseline_block"]),
        "timeline_file": timeline,
    })
}

fn write_data_files(runs: &[Run], out_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let data_dir = out_dir.join("data");
    let timelines_dir = data_dir.join("timelines");
    fs::create_dir_all(&timelines_dir)
        .map_err(|err| format!("could not create {}: {err}", timelines_dir.display()))?;
    let mut rows: Vec<Value> = Vec::new();
    let mut written = Vec::new();
    for run in runs {
        let target = run
            .result
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or("");
        if target == "midas" {
            for feed in run
                .result
                .get("feeds")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                rows.push(feed_row(run, Some(feed)));
                if let Some(file) = feed.get("timeline_file").and_then(Value::as_str) {
                    let source = run.dir.join(file);
                    let name = file.trim_start_matches("timelines/");
                    if let Ok(bytes) = fs::read(&source) {
                        let path = timelines_dir.join(name);
                        fs::write(&path, bytes)
                            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
                        written.push(path);
                    }
                }
            }
        } else {
            rows.push(feed_row(run, None));
        }
    }
    let feeds = serde_json::json!({
        "format": "crossfoot-feeds-v1",
        "rows": rows,
    });
    let path = data_dir.join("feeds.json");
    let mut text = serde_json::to_string_pretty(&feeds).map_err(|err| err.to_string())?;
    text.push('\n');
    fs::write(&path, text.as_bytes())
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    written.insert(0, path);
    Ok(written)
}

fn run_page(run: &Run) -> String {
    let result = &run.result;
    let target = result.get("target").and_then(Value::as_str).unwrap_or("");
    let verdict = verdict_of(result);
    let flag = verdict != "MODEL_MATCH" && verdict != "CONSISTENT";

    let mut html = String::new();
    html.push_str("<p class=\"back\"><a href=\"index.html\">Back to all runs</a></p>\n");
    html.push_str(&format!(
        "<h1>{} <span class=\"note\">{}</span></h1>\n",
        escape(target),
        escape(&run.name)
    ));
    html.push_str(&format!(
        "<p class=\"verdict{}\">verdict: {}</p>\n",
        if flag { " flag" } else { "" },
        escape(&verdict)
    ));
    html.push_str(&format!(
        "<p class=\"mono\">bundle root hash {}</p>\n",
        escape(run.root_hash.as_deref().unwrap_or("not sealed"))
    ));
    html.push_str(&format!(
        "<p class=\"lede\"><strong>Check class.</strong> {}</p>\n",
        escape(check_class_words(result))
    ));

    html.push_str("<h2>Window</h2>\n<p class=\"mono\">");
    html.push_str(&window_block(result));
    html.push_str("</p>\n");

    // Addresses.
    let mut addresses: Vec<(&str, String)> = Vec::new();
    if target == "svzchf" {
        addresses.push(("vault", crate::svzchf::VAULT.to_string()));
        addresses.push(("savings module", crate::svzchf::MODULE.to_string()));
    } else if target == "mtbill" {
        addresses.push(("token", crate::mtbill::TOKEN.to_string()));
        addresses.push(("oracle", crate::mtbill::ORACLE.to_string()));
        addresses.push(("dataFeed wrapper", crate::mtbill::DATA_FEED.to_string()));
        addresses.push(("deposit vault", crate::mtbill::DEPOSIT_VAULT.to_string()));
        addresses.push((
            "redemption vault",
            crate::mtbill::REDEMPTION_VAULT.to_string(),
        ));
        addresses.push((
            "redemption vault USTB",
            crate::mtbill::REDEMPTION_VAULT_USTB.to_string(),
        ));
        addresses.push(("access control", crate::mtbill::ACCESS_CONTROL.to_string()));
    }
    if !addresses.is_empty() {
        html.push_str("<h2>Addresses</h2>\n<dl class=\"kv\">\n");
        for (label, address) in addresses {
            html.push_str(&format!(
                "<dt>{}</dt><dd>{}</dd>\n",
                escape(label),
                escape(&address)
            ));
        }
        html.push_str("</dl>\n");
    }

    html.push_str(&match target {
        "svzchf" => svzchf_body(run),
        "mtbill" => mtbill_body(run),
        "midas" => midas_body(run),
        _ => String::new(),
    });

    // Unverified items, verbatim when the result carries any.
    let mut unverified: Vec<String> = Vec::new();
    for key in ["input_gaps", "stale_reads"] {
        if let Some(items) = result.get(key).and_then(Value::as_array) {
            for item in items {
                unverified.push(item.to_string());
            }
        }
    }
    html.push_str("<h2>Unverified</h2>\n");
    if unverified.is_empty() {
        html.push_str("<p class=\"note\">This result carries no input gaps or stale reads.</p>\n");
    } else {
        html.push_str("<ul>\n");
        for item in unverified {
            html.push_str(&format!("<li class=\"mono\">{}</li>\n", escape(&item)));
        }
        html.push_str("</ul>\n");
    }

    html.push_str(&provenance(run));
    html.push_str("<p class=\"back\"><a href=\"index.html\">Back to all runs</a></p>\n");
    page(&format!("{target} run, {}", run.name), &html)
}

fn index_page(runs: &[Run], skipped: &[String]) -> String {
    let mut html = String::new();
    html.push_str("<h1>Crossfoot, evidence bundles</h1>\n");
    html.push_str("<p class=\"lede\">Every number on these pages comes from the evidence bundle it links to, and every run is reproducible from the command line given on its page. Read the check class next to each verdict: the two targets are checked in different ways, and only one of them is a recomputation.</p>\n");

    html.push_str("<h2>Runs</h2>\n<table>\n<tr><th>target</th><th>window</th><th>check class</th><th>verdict</th><th>result</th><th></th></tr>\n");
    for run in runs {
        let verdict = verdict_of(&run.result);
        let flag = verdict != "MODEL_MATCH" && verdict != "CONSISTENT";
        html.push_str(&format!(
            "<tr><td>{}</td><td class=\"mono\">{}</td><td class=\"note\">{}</td><td class=\"verdict{}\">{}</td><td>{}</td><td><a href=\"{}.html\">open</a></td></tr>\n",
            escape(run.result.get("target").and_then(Value::as_str).unwrap_or("")),
            window_block(&run.result),
            escape(
                run.result
                    .get("check_class")
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ),
            if flag { " flag" } else { "" },
            escape(&verdict),
            escape(&headline(&run.result)),
            escape(&slug(&run.name))
        ));
    }
    html.push_str("</table>\n");

    html.push_str("<div class=\"panel\">\n<p class=\"note\"><strong>How to read the verdicts.</strong> MODEL_MATCH means every compared value was recomputed and matched the chain to the wei. CONSISTENT means the issuer's own rules check out against themselves; it is not a recomputation. OBSERVED_DEVIATION names a residual or a rule violation. MODEL_INCONSISTENT means the tool's two model paths disagreed with each other, so no statement about the chain is made. INSUFFICIENT_WINDOW means at least one check did not have enough data to run; it is not a pass. INPUT_GAP means a required input could not be observed at all, which for mTBILL's NAV is the permanent, expected state.</p>\n</div>\n");

    if !skipped.is_empty() {
        html.push_str(&format!(
            "<h2>Skipped</h2>\n<p class=\"note\">{} bundle director{} in the input contain no result.json and were skipped. Those are fetch bundles or partial runs: they hold raw responses and a manifest, but no verdict to render.</p>\n",
            skipped.len(),
            if skipped.len() == 1 { "y" } else { "ies" }
        ));
    }

    page("Crossfoot, evidence bundles", &html)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn render(bundles_dir: &Path, out_dir: &Path) -> Result<RenderOutcome, String> {
    let mut runs: Vec<Run> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    let mut dirs: Vec<PathBuf> = fs::read_dir(bundles_dir)
        .map_err(|err| format!("could not read {}: {err}", bundles_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let result_path = dir.join("result.json");
        if !result_path.is_file() {
            skipped.push(name);
            continue;
        }
        let raw = fs::read(&result_path)
            .map_err(|err| format!("could not read {}: {err}", result_path.display()))?;
        let result: Value = serde_json::from_slice(&raw)
            .map_err(|err| format!("{} is not JSON: {err}", result_path.display()))?;
        runs.push(Run {
            manifest: read_json(&dir.join("manifest.json")),
            result_sha256: crate::cache::sha256_hex(&raw),
            root_hash: fs::read_to_string(dir.join("bundle.sha256"))
                .ok()
                .map(|text| text.trim().to_string())
                .filter(|hash| hash.len() == 64),
            dir,
            name,
            result,
        });
    }

    // Deterministic order: target, then block, then name.
    runs.sort_by(|a, b| {
        let key = |run: &Run| {
            (
                run.result
                    .get("target")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                u64_at(&run.result, &["window", "block"]).unwrap_or(0),
                run.name.clone(),
            )
        };
        key(a).cmp(&key(b))
    });

    fs::create_dir_all(out_dir)
        .map_err(|err| format!("could not create {}: {err}", out_dir.display()))?;

    let mut pages = Vec::new();
    let index_path = out_dir.join("index.html");
    fs::write(&index_path, index_page(&runs, &skipped).as_bytes())
        .map_err(|err| format!("could not write index.html: {err}"))?;
    pages.push(index_path);

    for run in &runs {
        let path = out_dir.join(format!("{}.html", slug(&run.name)));
        fs::write(&path, run_page(run).as_bytes())
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        pages.push(path);
    }
    pages.extend(write_data_files(&runs, out_dir)?);

    Ok(RenderOutcome {
        out_dir: out_dir.to_path_buf(),
        rendered: runs.len(),
        pages,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn balanced(html: &str) -> Result<(), String> {
        const VOID: [&str; 11] = [
            "meta", "br", "hr", "img", "input", "line", "circle", "polyline", "path", "rect",
            "link",
        ];
        let mut stack: Vec<String> = Vec::new();
        let bytes: Vec<char> = html.chars().collect();
        let mut index = 0usize;
        while index < bytes.len() {
            if bytes[index] != '<' {
                index += 1;
                continue;
            }
            let end = match bytes[index..].iter().position(|c| *c == '>') {
                Some(offset) => index + offset,
                None => return Err("unterminated tag".to_string()),
            };
            let tag: String = bytes[index + 1..end].iter().collect();
            index = end + 1;
            if tag.starts_with('!') || tag.ends_with('/') {
                continue;
            }
            let closing = tag.starts_with('/');
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            if name.is_empty() || VOID.contains(&name.as_str()) {
                continue;
            }
            if closing {
                match stack.pop() {
                    Some(open) if open == name => {}
                    Some(open) => return Err(format!("closing <{name}> while <{open}> is open")),
                    None => return Err(format!("closing <{name}> with nothing open")),
                }
            } else {
                stack.push(name);
            }
        }
        if stack.is_empty() {
            Ok(())
        } else {
            Err(format!("unclosed tags: {stack:?}"))
        }
    }

    fn write_json_file(path: &Path, value: &Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    /// A 32 byte topic word for a small unsigned value.
    fn topic(value: u128) -> String {
        format!("0x{value:064x}")
    }

    /// Two synthetic run bundles, one per target, plus one bundle without a
    /// result.json so the skip path is exercised. Every number is invented;
    /// the fixture only has to carry the shape the renderer reads.
    fn synthetic_input(tag: &str) -> PathBuf {
        let input = std::env::temp_dir().join(format!("crossfoot-render-{tag}"));
        let _ = fs::remove_dir_all(&input);
        fs::create_dir_all(&input).unwrap();

        // svZCHF: a full recomputation with two recognition events.
        let svzchf = input.join("svzchf-run-100-200-20260101T000000Z");
        write_json_file(
            &svzchf.join("result.json"),
            &json!({
                "format": "crossfoot-result-v1",
                "target": "svzchf",
                "check_class": "full recomputation",
                "verdict": "MODEL_MATCH",
                "tolerance": "zero, to the wei",
                "window": {
                    "baseline_block": 100,
                    "baseline_timestamp_unix": 1_700_000_000u64,
                    "block": 200,
                    "block_timestamp_unix": 1_700_864_000u64,
                },
                "inputs": {
                    "rate_segments": [
                        { "start": 1_690_000_000u64, "rate_ppm": 30_000 },
                        { "start": 1_700_400_000u64, "rate_ppm": 40_000 },
                    ],
                    "uint40_segment_bound_violations": 0,
                    "recognition_events_in_window": 2,
                    "flow_events_total": 2,
                },
                "comparison": {
                    "check_class": "full recomputation",
                    "tolerance": "zero, to the wei",
                    "fields": [
                        { "field": "account.saved", "modeled": "1000000000000001000", "observed": "1000000000000001000", "residual": "0", "equal": true },
                        { "field": "account.ticks", "modeled": "123", "observed": "123", "residual": "0", "equal": true },
                    ],
                },
                "actus_cross_check": {
                    "recognition_points_compared": 2,
                    "horizon_compared": true,
                    "agree": true,
                    "divergences": [],
                    "smallest_distance_to_the_flooring_boundary_wei": "0.25",
                },
                "replay_steps": [
                    { "index": 0, "block": 150, "timestamp": 1_700_100_000u64, "action": "save", "amount": "1000000000000000000", "ticks_at_event": 10, "modeled_interest": "0", "state_after": { "saved": "1000000000000000000", "ticks": 50 } },
                    { "index": 1, "block": 170, "timestamp": 1_700_500_000u64, "action": "save", "amount": "1", "ticks_at_event": 60, "modeled_interest": "400", "observed_interest": "400", "state_after": { "saved": "1000000000000000401", "ticks": 70 } },
                    { "index": 2, "block": 190, "timestamp": 1_700_800_000u64, "action": "withdraw", "amount": "1", "ticks_at_event": 80, "modeled_interest": "600", "observed_interest": "600", "state_after": { "saved": "1000000000000001000", "ticks": 123 } },
                ],
                "stale_reads": [],
                "input_gaps": [],
            }),
        );
        write_json_file(
            &svzchf.join("manifest.json"),
            &json!({ "format": "crossfoot-manifest-v2", "entry_count": 32, "entries": [] }),
        );
        fs::write(
            svzchf.join("bundle.sha256"),
            "5d1a2fb0cc4bd0e3c0d5f3c2a1b3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1\n",
        )
        .unwrap();

        // mTBILL: a consistency bundle with three rounds, the first a
        // placeholder, the second labelled as the launch re-base.
        let mtbill = input.join("mtbill-run-100-200-20260101T000000Z");
        let rounds: [(u64, u128, u64, u64); 3] = [
            (1, 50_000_000_000, 1_700_050_000, 110),
            (2, 100_000_000, 1_700_100_000, 130),
            (3, 100_010_000, 1_700_700_000, 180),
        ];
        write_json_file(
            &mtbill
                .join("raw")
                .join("001-oracle-answerupdated-history.json"),
            &json!({
                "status": "1",
                "message": "OK",
                "result": rounds
                    .iter()
                    .map(|(round_id, answer, timestamp, block)| json!({
                        "topics": [topic(0), topic(*answer), topic(*round_id as u128), topic(*timestamp as u128)],
                        "blockNumber": format!("0x{block:x}"),
                    }))
                    .collect::<Vec<Value>>(),
            }),
        );
        write_json_file(
            &mtbill.join("manifest.json"),
            &json!({
                "format": "crossfoot-manifest-v1",
                "entry_count": 1,
                "entries": [
                    { "label": "oracle AnswerUpdated history", "file": "raw/001-oracle-answerupdated-history.json" },
                ],
            }),
        );
        write_json_file(
            &mtbill.join("result.json"),
            &json!({
                "format": "crossfoot-result-v1",
                "target": "mtbill",
                "check_class": "consistency",
                "nav_recomputation": "INPUT_GAP (underlying portfolio not observable)",
                "nav_recomputation_reason": "synthetic fixture",
                "consistency": "OBSERVED_DEVIATION",
                "failing_checks": ["C1"],
                "stale_checks": [],
                "input_gap_checks": [],
                "window": {
                    "baseline_block": 100,
                    "baseline_timestamp_unix": 1_700_000_000u64,
                    "block": 200,
                    "block_timestamp_unix": 1_700_864_000u64,
                },
                "checks": [
                    {
                        "id": "C1",
                        "name": "posting-rule replay",
                        "check_class": "consistency",
                        "verdict": "observed_deviation",
                        "summary": "3 rounds replayed, 1 violation(s)",
                        "detail": {},
                        "violations": [
                            {
                                "rule": "deviation from the previous answer within maxAnswerDeviation",
                                "round_id": 2,
                                "updated_at": 1_700_100_000u64,
                                "era": 0,
                                "classification": "launch rebase, not a manipulation signal: the feed was re-based from the pre-issue placeholder to the initial issue price",
                            },
                        ],
                    },
                    {
                        "id": "C8",
                        "name": "cross-source secondary price",
                        "check_class": "consistency",
                        "verdict": "informational",
                        "summary": "no secondary price was available",
                        "detail": {},
                        "violations": [],
                    },
                ],
                "posting_eras": [
                    { "index": 0, "from_block": 0 },
                    { "index": 1, "from_block": 150 },
                ],
                "attribution": {
                    "rounds": [
                        { "round_id": 2, "function": "setRoundData", "from": "0x0000000000000000000000000000000000000001" },
                    ],
                },
                "c1_c2_cross_reference": {
                    "answer": "synthetic answer",
                    "second_answer": "synthetic second answer",
                },
                "contract_sources": { "repo": "https://example.invalid/contracts", "commit": "0000000" },
                "input_gaps": [],
                "stale_reads": [],
            }),
        );

        // A fetch bundle without a result, to exercise the skip note.
        let fetch_only = input.join("svzchf-200-20260101T000000Z");
        write_json_file(
            &fetch_only.join("manifest.json"),
            &json!({ "format": "crossfoot-manifest-v1", "entry_count": 3, "entries": [] }),
        );
        input
    }

    #[test]
    fn renders_synthetic_bundles_and_skips_one_without_a_result() {
        let input = synthetic_input("synthetic");
        let out = std::env::temp_dir().join("crossfoot-render-synthetic-out");
        let _ = fs::remove_dir_all(&out);

        let outcome = render(&input, &out).unwrap();
        assert_eq!(outcome.rendered, 2, "both run bundles should render");
        assert_eq!(
            outcome.pages.len(),
            4,
            "index, one page per run, and data/feeds.json"
        );
        assert_eq!(
            outcome.skipped,
            vec!["svzchf-200-20260101T000000Z".to_string()]
        );

        let index = fs::read_to_string(out.join("index.html")).unwrap();
        assert!(
            index.contains("were skipped"),
            "the index should say a bundle was skipped"
        );

        for page in &outcome.pages {
            let html = fs::read_to_string(page).unwrap();
            balanced(&html)
                .unwrap_or_else(|err| panic!("{} is not well formed: {err}", page.display()));
            // No asset is fetched at view time. A URL in prose or an SVG
            // xmlns is not a fetch, so the check is for the constructs that
            // actually load something.
            for construct in ["<script", "<link", " src=", "url(http", "@import"] {
                assert!(
                    !html.contains(construct),
                    "{} must not load anything at view time, found {construct}",
                    page.display()
                );
            }
            // No colour that reads as a pass or fail signal.
            for forbidden in ["green", "#0f0", "#00ff00", "#ff0000", "lime", "crimson"] {
                assert!(
                    !html.to_lowercase().contains(forbidden),
                    "the page must not use a pass or fail colour ({forbidden})"
                );
            }
        }
    }

    #[test]
    fn rendering_twice_is_byte_identical() {
        let input = synthetic_input("identical");
        let first = std::env::temp_dir().join("crossfoot-render-a");
        let second = std::env::temp_dir().join("crossfoot-render-b");
        let _ = fs::remove_dir_all(&first);
        let _ = fs::remove_dir_all(&second);

        let a = render(&input, &first).unwrap();
        let b = render(&input, &second).unwrap();
        assert_eq!(a.pages.len(), b.pages.len());
        for (left, right) in a.pages.iter().zip(b.pages.iter()) {
            assert_eq!(
                left.file_name(),
                right.file_name(),
                "the page order must be deterministic"
            );
            assert_eq!(
                crate::cache::sha256_hex(&fs::read(left).unwrap()),
                crate::cache::sha256_hex(&fs::read(right).unwrap()),
                "{} differs between two renders",
                left.display()
            );
        }
    }

    /// The statements every page has to carry, asserted on the rendered
    /// output of the synthetic bundles.
    #[test]
    fn the_required_statements_are_on_the_page() {
        let input = synthetic_input("statements");
        let out = std::env::temp_dir().join("crossfoot-render-statements-out");
        let _ = fs::remove_dir_all(&out);
        render(&input, &out).unwrap();

        let mtbill =
            fs::read_to_string(out.join("mtbill-run-100-200-20260101t000000z.html")).unwrap();
        assert!(
            mtbill.contains("nav_recomputation: INPUT_GAP"),
            "the NAV input gap must be stated on its own line"
        );
        assert!(
            mtbill.contains("not an independent source"),
            "C8 must be marked as not independent"
        );
        assert!(
            mtbill.contains("launch rebase, not a manipulation signal"),
            "a round the result labels as the launch rebase must be labelled on the page"
        );
        assert!(
            mtbill.contains("Check class"),
            "the check class must sit next to the verdict"
        );
        assert!(mtbill.contains("All 3 posted rounds are counted"));
        // The placeholder round is drawn off scale, in the plain colour
        // because it is not itself a violation; the re-base round is circled.
        assert_eq!(mtbill.matches("l4,7 l-8,0 z").count(), 1);
        assert!(mtbill.contains("pre-issue placeholder"));
        assert!(
            !mtbill.contains(">500.0000<"),
            "the placeholder must not set the axis"
        );

        let svzchf =
            fs::read_to_string(out.join("svzchf-run-100-200-20260101t000000z.html")).unwrap();
        assert!(svzchf.contains("Full recomputation, zero tolerance"));
        // The run bundle is self contained: its own raw count is the whole
        // evidence, and nothing points at a bundle elsewhere.
        assert!(svzchf.contains("raw responses in this bundle"));
        assert!(svzchf.contains("<dd>32</dd>"));
        assert!(!svzchf.contains("referenced bundles"));
        // Spec 01 R9: the residual table columns, and the root hash next to
        // the verdict.
        assert!(svzchf.contains(
            "<tr><th>field</th><th>modeled</th><th>observed</th><th>residual</th><th>equal</th></tr>"
        ));
        assert!(svzchf.contains("<td class=\"mono\">true</td>"));
        let verdict_at = svzchf.find("verdict: MODEL_MATCH").unwrap();
        let hash_at = svzchf
            .find(
                "bundle root hash 5d1a2fb0cc4bd0e3c0d5f3c2a1b3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1",
            )
            .unwrap();
        assert!(hash_at > verdict_at && hash_at - verdict_at < 200);
        // A bundle without bundle.sha256 says so rather than showing nothing.
        assert!(mtbill.contains("bundle root hash not sealed"));
    }

    #[test]
    fn wei_renders_the_full_integer_with_a_point() {
        assert_eq!(wei("1005820467578421056", 18), "1.005820467578421056");
        assert_eq!(
            wei("81769497488003849675143", 18),
            "81769.497488003849675143"
        );
        assert_eq!(wei("0", 18), "0.000000000000000000");
        assert_eq!(wei("-5", 18), "-0.000000000000000005");
    }

    #[test]
    fn escaping_covers_the_markup_characters() {
        assert_eq!(escape("<a & \"b\">"), "&lt;a &amp; &quot;b&quot;&gt;");
    }

    #[test]
    fn the_balance_checker_catches_an_unbalanced_document() {
        assert!(balanced("<p>ok</p>").is_ok());
        assert!(balanced("<p><span>x</p></span>").is_err());
        assert!(balanced("<div>").is_err());
        assert!(balanced("<br><hr>").is_ok());
    }
    /// A1: one feeds.json row per run, carrying what the consumer reads.
    #[test]
    fn feeds_json_has_one_row_per_run() {
        let input = synthetic_input("feeds-json");
        let out = std::env::temp_dir().join("crossfoot-render-feeds-json-out");
        let _ = fs::remove_dir_all(&out);
        render(&input, &out).unwrap();
        let feeds: Value =
            serde_json::from_str(&fs::read_to_string(out.join("data/feeds.json")).unwrap())
                .unwrap();
        let rows = feeds["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "svzchf and mtbill, one row each");
        for row in rows {
            for key in [
                "address",
                "target",
                "product",
                "family",
                "verdict",
                "consumer_action",
                "nav_recomputation",
                "headline",
                "result_path",
                "block",
            ] {
                assert!(!row[key].is_null(), "{key} is set on {row}");
            }
            assert!(row.get("bundle_root").is_some());
            assert!(row.get("posting_path").is_some());
            assert!(row.get("liveness").is_some());
        }
        let svzchf = rows.iter().find(|r| r["target"] == "svzchf").unwrap();
        assert_eq!(svzchf["verdict"], "MODEL_MATCH");
        assert_eq!(svzchf["consumer_action"], "ALLOW");
        assert_eq!(svzchf["nav_recomputation"], "FULL");
        assert_eq!(svzchf["block"], 200);
        let mtbill = rows.iter().find(|r| r["target"] == "mtbill").unwrap();
        assert_eq!(mtbill["verdict"], "OBSERVED_DEVIATION");
        assert_eq!(mtbill["consumer_action"], "REVIEW");
        assert_eq!(mtbill["nav_recomputation"], "INPUT_GAP");
    }

    /// A3: the Midas run page draws the mRE7 timeline from the timeline file
    /// of the checked-in bundle, and the data export carries one row per
    /// feed plus a byte identical copy of every timeline file.
    #[test]
    fn midas_page_draws_the_timeline_from_the_timeline_file() {
        let fixture = crate::fixtures::midas_bundle();
        let input = std::env::temp_dir().join("crossfoot-render-midas-in");
        let _ = fs::remove_dir_all(&input);
        fs::create_dir_all(&input).unwrap();
        // Symlink the fixture in as one bundle directory.
        std::os::unix::fs::symlink(&fixture, input.join("midas-run-25884405")).unwrap();
        let out = std::env::temp_dir().join("crossfoot-render-midas-out");
        let _ = fs::remove_dir_all(&out);
        let outcome = render(&input, &out).unwrap();
        assert_eq!(outcome.rendered, 1);

        let html = fs::read_to_string(out.join("midas-run-25884405.html")).unwrap();
        balanced(&html).unwrap();
        assert!(html.contains("<h2>mRE7.customFeed</h2>"));
        assert!(
            html.contains("Drawn from timelines/mre7-customfeed.json in the bundle, 56 rounds.")
        );
        // The unchecked post in the accent colour with its hash as text.
        assert!(
            html.contains("r36 0x7579ba75b3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733")
        );
        assert!(html.contains("fill=\"var(--accent)\""));
        // The bound in force as a stepped line.
        assert!(html.contains("<title>bound in force</title>"));
        // The survey line, from summary only.
        assert!(html.contains("66 feeds replayed, 57 unchecked posts over the bound on 16 feeds"));
        assert!(html.contains("nav_recomputation: INPUT_GAP"));
        for construct in ["<script", "<link", " src=", "url(http", "@import"] {
            assert!(!html.contains(construct));
        }
        // Public wording: the technical identifier stays, the accusation does not.
        assert!(!html.to_lowercase().contains("bypassed the guard"));

        let index = fs::read_to_string(out.join("index.html")).unwrap();
        assert!(index.contains("66 feeds replayed, 57 unchecked posts over the bound on 16 feeds"));

        let feeds: Value =
            serde_json::from_str(&fs::read_to_string(out.join("data/feeds.json")).unwrap())
                .unwrap();
        let rows = feeds["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 66);
        let mre7 = rows
            .iter()
            .find(|r| r["product"] == "mRE7" && r["key"] == "customFeed")
            .unwrap();
        assert_eq!(
            mre7["address"],
            "0x0a2a51f2f206447dE3E3a80FCf92240244722395"
        );
        assert_eq!(mre7["verdict"], "OBSERVED_DEVIATION");
        assert_eq!(mre7["posting_path"], "ADMIN_GUARD_BYPASSED");
        assert_eq!(mre7["liveness"], "LIVE");
        assert_eq!(mre7["consumer_action"], "REVIEW");
        assert_eq!(mre7["nav_recomputation"], "INPUT_GAP");
        assert_eq!(mre7["family"], "guarded-setter");
        assert_eq!(mre7["block"], 25_884_405);
        assert_eq!(mre7["timeline_file"], "data/timelines/mre7-customfeed.json");
        assert_eq!(
            fs::read(out.join("data/timelines/mre7-customfeed.json")).unwrap(),
            fs::read(fixture.join("timelines/mre7-customfeed.json")).unwrap()
        );
        let derived = rows.iter().filter(|r| r["kind"] == "derived").count();
        assert_eq!(derived, 6);
    }
}
