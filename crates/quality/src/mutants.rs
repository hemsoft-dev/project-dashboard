//! Reads a cargo-mutants run (`mutants.out/outcomes.json`) and checks it against the mutation policy (#101).
//!
//! Every mutant ends in one class: caught, missed, timeout, unviable (it didn't build), or excluded. Excluded means
//! the platform doesn't build the function the mutant is in (from [`crate::source`]'s `cfg` walk), or the policy lists
//! it as an equivalent mutant with a reason. The score is caught / (caught + missed + timeout). A run that is
//! incomplete (its unmutated baseline failed, a mutant has no result, or a result is one this reader doesn't know) or
//! that has any timeout never passes.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::source::{Function, Platform};

/// A full run checks the score; a diff run (`cargo mutants --in-diff`) requires every mutant in the diff caught.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Full,
    Diff,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "full" => Some(Self::Full),
            "diff" => Some(Self::Diff),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Diff => "diff",
        }
    }
}

/// A mutant the tests can't catch because it doesn't change behavior. Matched exactly, never by pattern.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equivalent {
    /// The file, as cargo-mutants names it (`crates/dashboard-core/src/write.rs`).
    pub file: String,
    /// The mutant's description without its `file:line:col:` prefix (`replace || with && in Client::send`).
    pub mutant: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub platform: Platform,
    /// The files cargo-mutants examines (`.cargo/mutants.toml`).
    pub files: Vec<String>,
    /// A full run fails below this score, in percent.
    pub break_score: f64,
    /// The score to work toward, in percent. Reported, not enforced.
    pub target_score: f64,
    /// A full run that tested fewer mutants than this examined less than it should, and fails.
    pub min_mutants: usize,
    /// The first measurement the thresholds came from.
    pub baseline: Baseline,
    pub equivalent: Vec<Equivalent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub revision: String,
    pub counts: Counts,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub total: usize,
    pub caught: usize,
    pub missed: usize,
    pub timeout: usize,
    pub unviable: usize,
    pub excluded: usize,
    /// caught + missed + timeout.
    pub denominator: usize,
    /// caught / denominator × 100; 100 when the denominator is 0.
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mutant {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub class: String,
    /// Why it is excluded, for an excluded mutant.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub mode: Mode,
    pub platform: Platform,
    pub revision: String,
    pub cargo_mutants: String,
    pub counts: Counts,
    pub mutants: Vec<Mutant>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    Json(String),
    /// The run didn't finish or reports something this reader doesn't know: never a pass.
    Incomplete(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "outcomes.json: {e}"),
            Self::Incomplete(e) => write!(f, "incomplete mutation run: {e}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Classifies every mutant in `outcomes` (the text of `outcomes.json`), after checking it against `plan` (the text of
/// `mutants.json`, the mutants cargo-mutants set out to test). `functions` is the source walk of the crate root `prefix`
/// (`crates/dashboard-core/src`) for the policy's platform.
///
/// cargo-mutants rewrites `outcomes.json` after each result and counts `total_mutants` as it goes, so a run that stopped
/// part way looks whole by its own count (SFL on #114). Completeness is the plan: every planned mutant has exactly one
/// result, no result is unplanned, and the run recorded its `end_time`.
pub fn read(
    outcomes: &str,
    plan: &str,
    functions: &[Function],
    prefix: &str,
    policy: &Policy,
    mode: Mode,
    revision: &str,
) -> Result<Report, ReadError> {
    let value: Value = serde_json::from_str(outcomes).map_err(|e| ReadError::Json(e.to_string()))?;
    let version = value["cargo_mutants_version"].as_str().unwrap_or("unknown").to_string();
    let list = value["outcomes"]
        .as_array()
        .ok_or_else(|| ReadError::Json("no outcomes list".into()))?;
    let total = value["total_mutants"]
        .as_u64()
        .ok_or_else(|| ReadError::Json("no total_mutants".into()))? as usize;
    if !value["end_time"].is_string() {
        return Err(ReadError::Incomplete(
            "the run never finished: outcomes.json has no end_time".into(),
        ));
    }
    let mut baseline_ok = false;
    let mut mutants = Vec::new();
    for outcome in list {
        let summary = outcome["summary"].as_str().unwrap_or_default();
        match &outcome["scenario"] {
            Value::String(s) if s == "Baseline" => {
                if summary != "Success" {
                    return Err(ReadError::Incomplete(format!(
                        "the unmutated baseline ended {summary:?}"
                    )));
                }
                baseline_ok = true;
            }
            scenario => {
                let m = &scenario["Mutant"];
                let name = m["name"]
                    .as_str()
                    .ok_or_else(|| ReadError::Json("a mutant without a name".into()))?;
                let file = m["file"].as_str().unwrap_or_default().replace('\\', "/");
                let line = m["span"]["start"]["line"].as_u64().unwrap_or(0) as usize;
                let class = match summary {
                    "CaughtMutant" => "caught",
                    "MissedMutant" => "missed",
                    "Timeout" => "timeout",
                    "Unviable" => "unviable",
                    other => return Err(ReadError::Incomplete(format!("{name}: unknown result {other:?}"))),
                };
                mutants.push(Mutant {
                    name: name.to_string(),
                    file,
                    line,
                    class: class.to_string(),
                    reason: None,
                });
            }
        }
    }
    if !baseline_ok && !mutants.is_empty() {
        return Err(ReadError::Incomplete("no unmutated baseline result".into()));
    }
    if mutants.len() != total {
        return Err(ReadError::Incomplete(format!(
            "{} results for {total} mutants",
            mutants.len()
        )));
    }
    reconcile(plan, &mutants)?;
    let root = format!("{}/", prefix.trim_end_matches('/'));
    // An exclusion settles whether a result counts, never a timeout: a hang fails the gate wherever it happens (SFL on
    // #114).
    for m in mutants.iter_mut().filter(|m| m.class != "timeout") {
        if let Some(reason) = excluded(m, functions, &root, &policy.equivalent) {
            m.class = "excluded".to_string();
            m.reason = Some(reason);
        }
    }
    Ok(Report {
        mode,
        platform: policy.platform,
        revision: revision.to_string(),
        cargo_mutants: version,
        counts: count(&mutants),
        mutants,
    })
}

/// Every planned mutant has exactly one result and every result was planned.
fn reconcile(plan: &str, mutants: &[Mutant]) -> Result<(), ReadError> {
    let plan: Value = serde_json::from_str(plan).map_err(|e| ReadError::Json(format!("mutants.json: {e}")))?;
    let planned = plan
        .as_array()
        .ok_or_else(|| ReadError::Json("mutants.json is not a list".into()))?;
    let mut expected: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for m in planned {
        let name = m["name"]
            .as_str()
            .ok_or_else(|| ReadError::Json("a planned mutant without a name".into()))?;
        *expected.entry(name).or_default() += 1;
    }
    for m in mutants {
        match expected.get_mut(m.name.as_str()) {
            Some(n) if *n > 0 => *n -= 1,
            _ => {
                return Err(ReadError::Incomplete(format!(
                    "{}: a result for a mutant that wasn't planned",
                    m.name
                )));
            }
        }
    }
    let untested: Vec<&str> = expected
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(name, _)| *name)
        .collect();
    if let Some(first) = untested.first() {
        return Err(ReadError::Incomplete(format!(
            "{} planned mutants have no result, such as {first}",
            untested.len()
        )));
    }
    Ok(())
}
fn excluded(m: &Mutant, functions: &[Function], root: &str, equivalent: &[Equivalent]) -> Option<String> {
    let description = m.name.splitn(4, ':').nth(3).map(str::trim).unwrap_or(&m.name);
    if let Some(e) = equivalent.iter().find(|e| e.file == m.file && e.mutant == description) {
        return Some(format!("equivalent: {}", e.reason));
    }
    let rel = m.file.strip_prefix(root)?;
    // The innermost function holding the line decides, so a nested helper's own cfg counts.
    let owner = functions
        .iter()
        .filter(|f| f.file == rel && (f.start..=f.end).contains(&m.line))
        .min_by_key(|f| f.end - f.start)?;
    owner.excluded.as_ref().map(|why| format!("{} ({})", why, owner.name))
}

fn count(mutants: &[Mutant]) -> Counts {
    let n = |class: &str| mutants.iter().filter(|m| m.class == class).count();
    let (caught, missed, timeout) = (n("caught"), n("missed"), n("timeout"));
    let denominator = caught + missed + timeout;
    Counts {
        total: mutants.len(),
        caught,
        missed,
        timeout,
        unviable: n("unviable"),
        excluded: n("excluded"),
        denominator,
        score: if denominator == 0 {
            100.0
        } else {
            (caught as f64 * 10000.0 / denominator as f64).round() / 100.0
        },
    }
}

/// The reasons a report fails the policy; empty when it passes.
pub fn check(report: &Report, policy: &Policy) -> Vec<String> {
    let mut failures = Vec::new();
    let c = &report.counts;
    if report.platform != policy.platform {
        failures.push(format!(
            "policy is for {}, report is for {}",
            policy.platform.name(),
            report.platform.name()
        ));
    }
    if c.timeout > 0 {
        failures.push(format!("{} mutants timed out; a timeout is never a pass", c.timeout));
    }
    match report.mode {
        Mode::Full => {
            if c.total < policy.min_mutants {
                failures.push(format!(
                    "{} mutants tested, fewer than the policy's {}",
                    c.total, policy.min_mutants
                ));
            }
            if c.score < policy.break_score {
                failures.push(format!(
                    "mutation score {:.2}% ({} of {}) is below the break threshold {:.1}%",
                    c.score, c.caught, c.denominator, policy.break_score
                ));
            }
        }
        Mode::Diff => {
            for m in report.mutants.iter().filter(|m| m.class == "missed") {
                failures.push(format!("missed in the diff: {}", m.name));
            }
        }
    }
    failures
}

pub fn markdown(report: &Report, policy: &Policy, failures: &[String]) -> String {
    let c = &report.counts;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# dashboard-core mutation report ({}, {})\n",
        report.mode.name(),
        report.platform.name()
    );
    let _ = writeln!(s, "- Revision: `{}`", report.revision);
    let _ = writeln!(s, "- cargo-mutants {}", report.cargo_mutants);
    let _ = writeln!(
        s,
        "- Mutants: {} (caught {}, missed {}, timeout {}, unviable {}, excluded {})",
        c.total, c.caught, c.missed, c.timeout, c.unviable, c.excluded
    );
    let _ = writeln!(
        s,
        "- Score: {:.2}% = caught {} / (caught + missed + timeout) {}",
        c.score, c.caught, c.denominator
    );
    if report.mode == Mode::Full {
        let _ = writeln!(
            s,
            "- Break {:.1}%, target {:.1}% ({})",
            policy.break_score,
            policy.target_score,
            if c.score >= policy.target_score {
                "met"
            } else {
                "not yet"
            }
        );
    }
    let _ = writeln!(s, "- Gate: {}\n", if failures.is_empty() { "pass" } else { "FAIL" });
    let mut section = |title: &str, items: Vec<String>| {
        let _ = writeln!(s, "## {title} ({})\n", items.len());
        for i in items {
            let _ = writeln!(s, "- {i}");
        }
        let _ = writeln!(s);
    };
    section("Failures", failures.to_vec());
    let by = |class: &str| -> Vec<String> {
        report
            .mutants
            .iter()
            .filter(|m| m.class == class)
            .map(|m| match &m.reason {
                Some(r) => format!("{}: {r}", m.name),
                None => m.name.clone(),
            })
            .collect()
    };
    section("Missed", by("missed"));
    section("Timeout", by("timeout"));
    section("Excluded", by("excluded"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Excluded;

    const FILE: &str = "crates/x/src/write.rs";

    fn outcomes(baseline: &str, mutants: &[(&str, usize, &str)], total: Option<usize>) -> String {
        let mut list = vec![serde_json::json!({"scenario": "Baseline", "summary": baseline})];
        for (description, line, summary) in mutants {
            list.push(serde_json::json!({
                "scenario": {"Mutant": {
                    "name": format!("{FILE}:{line}:5: {description}"),
                    "file": FILE,
                    "span": {"start": {"line": line, "column": 5}, "end": {"line": line, "column": 9}},
                }},
                "summary": summary,
            }));
        }
        serde_json::json!({
            "cargo_mutants_version": "27.1.0",
            "total_mutants": total.unwrap_or(mutants.len()),
            "end_time": "2026-11-19T06:28:02Z",
            "outcomes": list,
        })
        .to_string()
    }

    fn function(name: &str, start: usize, end: usize, excluded: Option<&str>) -> Function {
        Function {
            key: format!("write.rs::{name}"),
            file: "write.rs".into(),
            name: name.into(),
            start,
            end,
            nested: vec![],
            complexity: 1,
            excluded: excluded.map(|c| Excluded::NotBuilt(c.into())),
        }
    }

    fn policy(break_score: f64, equivalent: Vec<Equivalent>) -> Policy {
        Policy {
            platform: Platform::Linux,
            files: vec![FILE.into()],
            break_score,
            target_score: 95.0,
            min_mutants: 1,
            baseline: Baseline {
                revision: "b".repeat(40),
                counts: Counts::default(),
            },
            equivalent,
        }
    }

    /// The plan cargo-mutants would have written for exactly the mutants in `text`.
    fn plan_of(text: &str) -> String {
        let value: Value = serde_json::from_str(text).unwrap_or_default();
        let names: Vec<Value> = value["outcomes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| o["scenario"]["Mutant"]["name"].as_str())
            .map(|name| serde_json::json!({"name": name}))
            .collect();
        Value::Array(names).to_string()
    }

    fn report(text: &str, p: &Policy, mode: Mode) -> Result<Report, ReadError> {
        report_with_plan(text, &plan_of(text), p, mode)
    }

    fn report_with_plan(text: &str, plan: &str, p: &Policy, mode: Mode) -> Result<Report, ReadError> {
        let functions = [function("send", 1, 50, None), function("acl", 60, 80, Some("windows"))];
        read(text, plan, &functions, "crates/x/src", p, mode, &"a".repeat(40))
    }

    #[test]
    fn counts_each_class_and_scores_caught_over_caught_missed_and_timeout() {
        let text = outcomes(
            "Success",
            &[
                ("replace + with - in send", 10, "CaughtMutant"),
                ("replace + with * in send", 10, "CaughtMutant"),
                ("replace > with >= in send", 11, "CaughtMutant"),
                ("replace send with ()", 12, "MissedMutant"),
                ("replace send -> u8 with 0", 13, "Unviable"),
            ],
            None,
        );
        let r = report(&text, &policy(0.0, vec![]), Mode::Full).unwrap();
        assert_eq!(
            r.counts,
            Counts {
                total: 5,
                caught: 3,
                missed: 1,
                timeout: 0,
                unviable: 1,
                excluded: 0,
                denominator: 4,
                score: 75.0
            }
        );
        assert_eq!(r.cargo_mutants, "27.1.0");
    }

    #[test]
    fn a_mutant_in_code_the_platform_does_not_build_and_a_listed_equivalent_are_excluded() {
        let text = outcomes(
            "Success",
            &[
                ("replace acl with ()", 70, "MissedMutant"),
                ("replace || with && in send", 20, "MissedMutant"),
                ("replace || with && in send", 21, "CaughtMutant"),
                ("replace == with != in send", 22, "MissedMutant"),
            ],
            None,
        );
        let equivalent = Equivalent {
            file: FILE.into(),
            mutant: "replace || with && in send".into(),
            reason: "both conditions hold for every real timeout".into(),
        };
        let r = report(&text, &policy(0.0, vec![equivalent]), Mode::Full).unwrap();
        assert_eq!((r.counts.excluded, r.counts.missed, r.counts.caught), (3, 1, 0));
        assert_eq!(
            r.mutants[0].reason.as_deref(),
            Some("not built on this platform: cfg(windows) (acl)")
        );
        assert_eq!(
            r.mutants[1].reason.as_deref(),
            Some("equivalent: both conditions hold for every real timeout")
        );
        // An exact description excludes every mutant it names, caught or not; any other description stays.
        assert_eq!(r.mutants[2].class, "excluded");
        assert_eq!(r.mutants[3].class, "missed");
    }

    #[test]
    fn a_timeout_in_excluded_code_still_fails_the_gate() {
        // SFL on #114, round 2: the platform exclusion turned a timeout into "excluded", so the gate never saw it.
        let equivalent = Equivalent {
            file: FILE.into(),
            mutant: "replace == with != in send".into(),
            reason: "no behavior change".into(),
        };
        let p = policy(0.0, vec![equivalent]);
        let text = outcomes(
            "Success",
            &[
                ("replace acl with ()", 70, "Timeout"),
                ("replace == with != in send", 20, "Timeout"),
                ("replace + with - in send", 10, "CaughtMutant"),
            ],
            None,
        );
        let r = report(&text, &p, Mode::Full).unwrap();
        assert_eq!((r.counts.timeout, r.counts.excluded, r.counts.caught), (2, 0, 1));
        assert!(r.mutants.iter().all(|m| m.reason.is_none()));
        assert_eq!(check(&r, &p), ["2 mutants timed out; a timeout is never a pass"]);
    }
    #[test]
    fn an_incomplete_run_is_an_error_never_a_report() {
        let p = policy(0.0, vec![]);
        let one = [("replace send with ()", 12, "CaughtMutant")];
        assert_eq!(
            report(&outcomes("Failure", &one, None), &p, Mode::Full).unwrap_err(),
            ReadError::Incomplete("the unmutated baseline ended \"Failure\"".into())
        );
        assert_eq!(
            report(&outcomes("Success", &one, Some(2)), &p, Mode::Full).unwrap_err(),
            ReadError::Incomplete("1 results for 2 mutants".into())
        );
        let odd = [("replace send with ()", 12, "Success")];
        assert!(matches!(
            report(&outcomes("Success", &odd, None), &p, Mode::Full),
            Err(ReadError::Incomplete(_))
        ));
        assert!(matches!(report("{}", &p, Mode::Full), Err(ReadError::Json(_))));
        assert!(matches!(report("not json", &p, Mode::Full), Err(ReadError::Json(_))));
    }

    #[test]
    fn a_run_that_stopped_part_way_is_incomplete_though_its_own_count_agrees() {
        // SFL on #114: cargo-mutants counts total_mutants as results land, so only the plan and end_time can tell.
        let p = policy(0.0, vec![]);
        let done = [("replace + with - in send", 10, "CaughtMutant")];
        let text = outcomes("Success", &done, None);
        let two_planned = serde_json::json!([
            {"name": format!("{FILE}:10:5: replace + with - in send")},
            {"name": format!("{FILE}:11:5: replace > with >= in send")},
        ])
        .to_string();
        let err = report_with_plan(&text, &two_planned, &p, Mode::Full).unwrap_err();
        assert_eq!(
            err,
            ReadError::Incomplete(format!(
                "1 planned mutants have no result, such as {FILE}:11:5: replace > with >= in send"
            ))
        );

        let mut unfinished: Value = serde_json::from_str(&text).unwrap();
        unfinished.as_object_mut().unwrap().remove("end_time");
        let err = report_with_plan(&unfinished.to_string(), &plan_of(&text), &p, Mode::Full).unwrap_err();
        assert_eq!(
            err,
            ReadError::Incomplete("the run never finished: outcomes.json has no end_time".into())
        );

        let err = report_with_plan(&text, "[]", &p, Mode::Diff).unwrap_err();
        assert!(matches!(err, ReadError::Incomplete(e) if e.ends_with("a result for a mutant that wasn't planned")));
        assert!(matches!(
            report_with_plan(&text, "{}", &p, Mode::Full),
            Err(ReadError::Json(_))
        ));
    }
    #[test]
    fn a_full_run_fails_below_the_break_threshold_on_any_timeout_or_with_too_few_mutants() {
        let text = outcomes(
            "Success",
            &[
                ("replace + with - in send", 10, "CaughtMutant"),
                ("replace send with ()", 12, "MissedMutant"),
            ],
            None,
        );
        let r = report(&text, &policy(50.0, vec![]), Mode::Full).unwrap();
        assert!(check(&r, &policy(50.0, vec![])).is_empty(), "50% meets a 50% break");
        let failures = check(&r, &policy(50.1, vec![]));
        assert_eq!(
            failures,
            ["mutation score 50.00% (1 of 2) is below the break threshold 50.1%"]
        );

        let mut few = policy(0.0, vec![]);
        few.min_mutants = 3;
        assert_eq!(check(&r, &few), ["2 mutants tested, fewer than the policy's 3"]);

        let timed = outcomes("Success", &[("replace + with - in send", 10, "Timeout")], None);
        let r = report(&timed, &policy(0.0, vec![]), Mode::Full).unwrap();
        assert_eq!(r.counts.score, 0.0);
        assert!(check(&r, &policy(0.0, vec![]))[0].contains("timed out; a timeout is never a pass"));
    }

    #[test]
    fn a_diff_run_needs_every_mutant_caught_and_passes_with_none() {
        let p = policy(99.0, vec![]);
        let text = outcomes(
            "Success",
            &[
                ("replace + with - in send", 10, "CaughtMutant"),
                ("replace send with ()", 12, "MissedMutant"),
            ],
            None,
        );
        let r = report(&text, &p, Mode::Diff).unwrap();
        assert_eq!(
            check(&r, &p),
            [format!("missed in the diff: {FILE}:12:5: replace send with ()")]
        );
        // A diff that touches no examined code has nothing to mutate; cargo-mutants then writes no baseline result.
        let none =
            serde_json::json!({"cargo_mutants_version": "27.1.0", "total_mutants": 0, "end_time": "x", "outcomes": []})
                .to_string();
        let r = report(&none, &p, Mode::Diff).unwrap();
        assert_eq!(r.counts.score, 100.0);
        assert!(check(&r, &p).is_empty());
    }

    #[test]
    fn a_policy_for_another_platform_fails() {
        let text = outcomes("Success", &[("replace + with - in send", 10, "CaughtMutant")], None);
        let linux = policy(0.0, vec![]);
        let r = report(&text, &linux, Mode::Full).unwrap();
        let mut windows = linux.clone();
        windows.platform = Platform::Windows;
        assert_eq!(check(&r, &windows), ["policy is for windows, report is for linux"]);
    }

    #[test]
    fn the_markdown_names_the_revision_counts_denominator_and_gate() {
        let text = outcomes("Success", &[("replace send with ()", 12, "MissedMutant")], None);
        let p = policy(50.0, vec![]);
        let r = report(&text, &p, Mode::Full).unwrap();
        let md = markdown(&r, &p, &check(&r, &p));
        assert!(md.contains(&format!("- Revision: `{}`", "a".repeat(40))));
        assert!(md.contains("- Mutants: 1 (caught 0, missed 1, timeout 0, unviable 0, excluded 0)"));
        assert!(md.contains("- Score: 0.00% = caught 0 / (caught + missed + timeout) 1"));
        assert!(md.contains("- Gate: FAIL"));
        assert!(md.contains("## Missed (1)"));
    }
}
