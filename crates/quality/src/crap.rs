//! Joins functions with their line coverage, computes CRAP, and checks it against a baseline.
//!
//! `CRAP = complexity² × (1 − coverage)³ + complexity`, with coverage in 0..1. A function every line of which runs
//! scores its complexity; an unrun one scores complexity² + complexity.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::lcov::Lcov;
use crate::source::{Function, Platform};

/// Above this a new function fails the gate.
pub const CRAP_LIMIT: f64 = 30.0;
/// Above this a function is listed for review.
pub const REVIEW_FROM: f64 = 15.0;
/// What the coverage numbers count. Branch coverage needs a nightly compiler (`-Z coverage-options=branch`); the
/// toolchain is stable, so lines are the basis (#100).
pub const BASIS: &str = "lines (LLVM source-based coverage, stable rustc; no branch coverage)";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Measured {
    pub key: String,
    pub file: String,
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub complexity: u32,
    /// Instrumented lines the function owns, and how many of them ran.
    pub lines: usize,
    pub covered: usize,
    /// 0..1; None when excluded.
    pub coverage: Option<f64>,
    pub crap: Option<f64>,
    /// Why there is no coverage to read on this platform.
    pub excluded: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Totals {
    pub functions: usize,
    pub measured: usize,
    pub excluded: usize,
    pub lines: usize,
    pub covered: usize,
    /// covered / lines × 100, over the measured functions' own lines.
    pub percent: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub platform: Platform,
    pub revision: String,
    pub basis: String,
    pub totals: Totals,
    pub functions: Vec<Measured>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JoinError {
    /// A built function's file isn't in the coverage report: the report doesn't describe this source.
    FileMissing(String),
    NoFunctions,
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileMissing(file) => write!(
                f,
                "the coverage report has no record for {file}, which has built functions"
            ),
            Self::NoFunctions => write!(f, "no production functions found"),
        }
    }
}

impl std::error::Error for JoinError {}

pub fn crap(complexity: u32, coverage: f64) -> f64 {
    let c = f64::from(complexity);
    c * c * (1.0 - coverage).powi(3) + c
}

/// `prefix` is the source root's path relative to the repository (`crates/dashboard-core/src`), which LCOV paths end
/// with.
pub fn join(
    functions: &[Function],
    lcov: &Lcov,
    prefix: &str,
    platform: Platform,
    revision: &str,
) -> Result<Report, JoinError> {
    if functions.is_empty() {
        return Err(JoinError::NoFunctions);
    }
    let mut out = Vec::new();
    for f in functions {
        let mut m = Measured {
            key: f.key.clone(),
            file: f.file.clone(),
            name: f.name.clone(),
            start: f.start,
            end: f.end,
            complexity: f.complexity,
            lines: 0,
            covered: 0,
            coverage: None,
            crap: None,
            excluded: f.excluded.as_ref().map(ToString::to_string),
        };
        if m.excluded.is_none() {
            let path = format!("{}/{}", prefix.trim_end_matches('/'), f.file);
            let hits = lcov.file(&path).ok_or_else(|| JoinError::FileMissing(path.clone()))?;
            for (_, &count) in hits.range(f.start..=f.end).filter(|(line, _)| f.owns(**line)) {
                m.lines += 1;
                if count > 0 {
                    m.covered += 1;
                }
            }
            // A built function with no instrumented line never ran: zero, not unknown.
            let coverage = if m.lines == 0 {
                0.0
            } else {
                m.covered as f64 / m.lines as f64
            };
            m.coverage = Some(coverage);
            m.crap = Some(round2(crap(f.complexity, coverage)));
        }
        out.push(m);
    }
    let measured: Vec<&Measured> = out.iter().filter(|m| m.excluded.is_none()).collect();
    let lines: usize = measured.iter().map(|m| m.lines).sum();
    let covered: usize = measured.iter().map(|m| m.covered).sum();
    let totals = Totals {
        functions: out.len(),
        measured: measured.len(),
        excluded: out.len() - measured.len(),
        lines,
        covered,
        percent: if lines == 0 {
            0.0
        } else {
            round2(covered as f64 * 100.0 / lines as f64)
        },
    };
    Ok(Report {
        platform,
        revision: revision.to_string(),
        basis: BASIS.to_string(),
        totals,
        functions: out,
    })
}

/// The accepted measurement a candidate must not fall below.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Baseline {
    pub platform: Platform,
    /// The revision it was measured at.
    pub revision: String,
    /// The lowest line coverage, in percent, that passes.
    pub line_coverage_floor: f64,
    /// CRAP per function key, as measured.
    pub crap: BTreeMap<String, f64>,
}

impl Baseline {
    /// A baseline from this report: the floor is its coverage rounded down to a tenth of a percent.
    pub fn from_report(report: &Report) -> Self {
        Self {
            platform: report.platform,
            revision: report.revision.clone(),
            line_coverage_floor: (report.totals.percent * 10.0).floor() / 10.0,
            crap: report
                .functions
                .iter()
                .filter_map(|m| Some((m.key.clone(), m.crap?)))
                .collect(),
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Verdict {
    pub failures: Vec<String>,
    /// Functions scoring above REVIEW_FROM and at most CRAP_LIMIT.
    pub review: Vec<String>,
    /// Functions above CRAP_LIMIT that the baseline already had at or above their score.
    pub legacy: Vec<String>,
}

impl Verdict {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

pub fn check(report: &Report, baseline: &Baseline) -> Verdict {
    let mut v = Verdict::default();
    if baseline.platform != report.platform {
        v.failures.push(format!(
            "baseline is for {}, report is for {}",
            baseline.platform.name(),
            report.platform.name()
        ));
        return v;
    }
    if report.totals.measured == 0 {
        v.failures.push("no function was measured".to_string());
        return v;
    }
    if report.totals.percent < baseline.line_coverage_floor {
        v.failures.push(format!(
            "line coverage {:.2}% is below the floor {:.1}% ({} of {} lines)",
            report.totals.percent, baseline.line_coverage_floor, report.totals.covered, report.totals.lines
        ));
    }
    for m in &report.functions {
        let Some(score) = m.crap else { continue };
        let at = format!(
            "{} ({}:{}, complexity {}, coverage {:.0}%, CRAP {score:.2})",
            m.name,
            m.file,
            m.start,
            m.complexity,
            m.coverage.unwrap_or(0.0) * 100.0
        );
        if score > CRAP_LIMIT {
            match baseline.crap.get(&m.key) {
                Some(&was) if was > CRAP_LIMIT && score <= was + 0.005 => v.legacy.push(at),
                Some(&was) if was > CRAP_LIMIT => v.failures.push(format!("{at}: up from {was:.2}")),
                Some(&was) => v.failures.push(format!("{at}: above {CRAP_LIMIT}, was {was:.2}")),
                None => v.failures.push(format!("{at}: new function above {CRAP_LIMIT}")),
            }
        } else if score > REVIEW_FROM {
            v.review.push(at);
        }
    }
    v
}

pub fn markdown(report: &Report, verdict: &Verdict, worst: usize) -> String {
    let t = &report.totals;
    let mut s = String::new();
    let _ = writeln!(s, "# dashboard-core CRAP report ({})\n", report.platform.name());
    let _ = writeln!(s, "- Revision: `{}`", report.revision);
    let _ = writeln!(s, "- Coverage basis: {}", report.basis);
    let _ = writeln!(
        s,
        "- Functions: {} ({} measured, {} excluded)",
        t.functions, t.measured, t.excluded
    );
    let _ = writeln!(
        s,
        "- Line coverage: {:.2}% ({} of {} lines)",
        t.percent, t.covered, t.lines
    );
    let _ = writeln!(s, "- Gate: {}\n", if verdict.passed() { "pass" } else { "FAIL" });
    let section = |s: &mut String, title: &str, items: &[String]| {
        let _ = writeln!(s, "## {title} ({})\n", items.len());
        for i in items {
            let _ = writeln!(s, "- {i}");
        }
        let _ = writeln!(s);
    };
    section(&mut s, "Failures", &verdict.failures);
    section(
        &mut s,
        &format!("Legacy above {CRAP_LIMIT}, not increased"),
        &verdict.legacy,
    );
    section(
        &mut s,
        &format!("Review queue, CRAP {REVIEW_FROM}..{CRAP_LIMIT}"),
        &verdict.review,
    );
    let mut ranked: Vec<&Measured> = report.functions.iter().filter(|m| m.crap.is_some()).collect();
    ranked.sort_by(|a, b| {
        b.crap
            .partial_cmp(&a.crap)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.key.cmp(&b.key))
    });
    let _ = writeln!(
        s,
        "## Worst {}\n\n| CRAP | Complexity | Coverage | Function |\n| ---: | ---: | ---: | --- |",
        worst.min(ranked.len())
    );
    for m in ranked.iter().take(worst) {
        let _ = writeln!(
            s,
            "| {:.2} | {} | {:.0}% | `{}` {}:{} |",
            m.crap.unwrap_or(0.0),
            m.complexity,
            m.coverage.unwrap_or(0.0) * 100.0,
            m.name,
            m.file,
            m.start
        );
    }
    let excluded: Vec<String> = report
        .functions
        .iter()
        .filter_map(|m| Some(format!("`{}` {}:{}: {}", m.name, m.file, m.start, m.excluded.as_ref()?)))
        .collect();
    let _ = writeln!(s);
    section(&mut s, "Excluded", &excluded);
    s
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Excluded;

    fn function(key: &str, start: usize, end: usize, complexity: u32) -> Function {
        Function {
            key: key.into(),
            file: "lib.rs".into(),
            name: key.into(),
            start,
            end,
            nested: vec![],
            complexity,
            excluded: None,
        }
    }

    fn lcov(lines: &[(usize, u64)]) -> Lcov {
        let mut text = String::from("SF:/repo/crates/x/src/lib.rs\n");
        for (l, n) in lines {
            text += &format!("DA:{l},{n}\n");
        }
        Lcov::parse(&(text + "end_of_record\n")).unwrap()
    }

    fn report(functions: &[Function], lines: &[(usize, u64)]) -> Report {
        join(
            functions,
            &lcov(lines),
            "crates/x/src",
            Platform::Linux,
            &"a".repeat(40),
        )
        .unwrap()
    }

    fn baseline(floor: f64, crap: &[(&str, f64)]) -> Baseline {
        Baseline {
            platform: Platform::Linux,
            revision: "b".repeat(40),
            line_coverage_floor: floor,
            crap: crap.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
        }
    }

    #[test]
    fn crap_is_complexity_alone_when_covered_and_its_square_plus_itself_when_not() {
        assert_eq!(crap(5, 1.0), 5.0);
        assert_eq!(crap(5, 0.0), 30.0);
        assert!((crap(10, 0.5) - 22.5).abs() < 1e-9);
    }

    #[test]
    fn join_counts_only_the_functions_own_instrumented_lines() {
        let mut outer = function("outer", 1, 10, 2);
        outer.nested = vec![(4, 6)];
        let r = report(
            &[outer, function("helper", 4, 6, 1)],
            &[(1, 1), (2, 0), (5, 3), (10, 1), (20, 9)],
        );
        let (o, h) = (&r.functions[0], &r.functions[1]);
        assert_eq!((o.lines, o.covered), (3, 2));
        assert_eq!((h.lines, h.covered), (1, 1));
        assert_eq!((r.totals.lines, r.totals.covered, r.totals.percent), (4, 3, 75.0));
    }

    #[test]
    fn a_built_function_without_instrumented_lines_is_uncovered_and_an_excluded_one_is_left_out() {
        let mut gone = function("gone", 30, 31, 3);
        gone.excluded = Some(Excluded::NotBuilt("windows".into()));
        let r = report(&[function("never", 1, 2, 3), gone], &[(9, 1)]);
        assert_eq!(r.functions[0].coverage, Some(0.0));
        assert_eq!(r.functions[0].crap, Some(12.0));
        assert_eq!(
            (r.functions[1].crap, r.functions[1].excluded.as_deref()),
            (None, Some("not built on this platform: cfg(windows)"))
        );
        assert_eq!((r.totals.measured, r.totals.excluded), (1, 1));
    }

    #[test]
    fn a_report_without_the_functions_file_is_an_error() {
        let err = join(
            &[function("f", 1, 2, 1)],
            &lcov(&[(1, 1)]),
            "crates/other/src",
            Platform::Linux,
            "x",
        )
        .unwrap_err();
        assert_eq!(err, JoinError::FileMissing("crates/other/src/lib.rs".into()));
        assert_eq!(
            join(&[], &lcov(&[]), "crates/x/src", Platform::Linux, "x").unwrap_err(),
            JoinError::NoFunctions
        );
    }

    #[test]
    fn the_baseline_floor_rounds_down_to_a_tenth() {
        let r = report(&[function("f", 1, 3, 1)], &[(1, 1), (2, 1), (3, 0)]);
        assert_eq!(r.totals.percent, 66.67);
        assert_eq!(Baseline::from_report(&r).line_coverage_floor, 66.6);
        assert_eq!(Baseline::from_report(&r).crap["f"], 1.04);
    }

    #[test]
    fn the_measured_baseline_passes_and_lists_its_review_queue_and_legacy() {
        // complexity 6 uncovered = 42; complexity 4 uncovered = 20; complexity 2 covered = 2.
        let fs = [
            function("legacy", 1, 1, 6),
            function("review", 2, 2, 4),
            function("fine", 3, 3, 2),
        ];
        let r = report(&fs, &[(1, 0), (2, 0), (3, 1)]);
        let v = check(&r, &Baseline::from_report(&r));
        assert!(v.passed(), "{v:?}");
        assert_eq!(v.legacy.len(), 1);
        assert!(v.legacy[0].starts_with("legacy (lib.rs:1, complexity 6, coverage 0%, CRAP 42.00)"));
        assert_eq!(v.review.len(), 1);
    }

    #[test]
    fn coverage_below_the_floor_fails() {
        let r = report(&[function("f", 1, 2, 1)], &[(1, 1), (2, 0)]);
        let v = check(&r, &baseline(50.1, &[("f", 1.13)]));
        assert_eq!(
            v.failures,
            ["line coverage 50.00% is below the floor 50.1% (1 of 2 lines)"]
        );
    }

    #[test]
    fn a_new_function_above_the_limit_fails() {
        let r = report(&[function("big", 1, 1, 6)], &[(1, 0)]);
        let v = check(&r, &baseline(0.0, &[]));
        assert_eq!(v.failures.len(), 1);
        assert!(v.failures[0].ends_with("new function above 30"), "{:?}", v.failures);
    }

    #[test]
    fn an_existing_function_crossing_the_limit_or_a_legacy_one_growing_fails() {
        let r = report(
            &[function("crossed", 1, 1, 6), function("grew", 2, 2, 7)],
            &[(1, 0), (2, 0)],
        );
        let v = check(&r, &baseline(0.0, &[("crossed", 6.0), ("grew", 42.0)]));
        assert_eq!(v.failures.len(), 2, "{:?}", v.failures);
        assert!(v.failures[0].ends_with("above 30, was 6.00"));
        assert!(v.failures[1].ends_with("up from 42.00"));
    }

    #[test]
    fn a_legacy_function_that_improves_passes() {
        // complexity 8 at 25%: 64 × 0.42 + 8 = 35, still above the limit but below its baseline 42.
        let r = report(&[function("legacy", 1, 4, 8)], &[(1, 1), (2, 0), (3, 0), (4, 0)]);
        assert_eq!(r.functions[0].crap, Some(35.0));
        assert!(check(&r, &baseline(0.0, &[("legacy", 42.0)])).passed());
    }

    #[test]
    fn a_baseline_for_another_platform_or_nothing_measured_fails() {
        let r = report(&[function("f", 1, 1, 1)], &[(1, 1)]);
        let mut other = Baseline::from_report(&r);
        other.platform = Platform::Windows;
        assert_eq!(
            check(&r, &other).failures,
            ["baseline is for windows, report is for linux"]
        );

        let mut gone = function("f", 1, 1, 1);
        gone.excluded = Some(Excluded::NotBuilt("windows".into()));
        let empty = report(&[gone], &[(1, 1)]);
        assert_eq!(
            check(&empty, &baseline(0.0, &[])).failures,
            ["no function was measured"]
        );
    }

    #[test]
    fn the_markdown_names_the_revision_basis_and_failures() {
        let r = report(&[function("big", 1, 1, 6)], &[(1, 0)]);
        let md = markdown(&r, &check(&r, &baseline(0.0, &[])), 5);
        assert!(md.contains(&format!("- Revision: `{}`", "a".repeat(40))));
        assert!(md.contains("no branch coverage"));
        assert!(md.contains("- Gate: FAIL"));
        assert!(md.contains("| 42.00 | 6 | 0% | `big` lib.rs:1 |"));
    }
}
