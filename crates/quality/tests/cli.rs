//! The `quality crap` command end to end on the fixture crate, with hand-written LCOV: the measured baseline passes,
//! removing coverage or adding a function above the CRAP limit fails with exit 1, and bad input exits 2.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

struct Run {
    dir: PathBuf,
}

impl Run {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("quality-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    /// A copy of the fixture source under `crates/demo/src`, so a test can change it.
    fn source(&self) -> PathBuf {
        let src = self.dir.join("crates/demo/src");
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crate/src"),
            &src,
        );
        src
    }

    /// LCOV covering every line of every fixture file, except `uncovered` lines of lib.rs.
    fn lcov(&self, src: &Path, uncovered: std::ops::RangeInclusive<usize>) -> PathBuf {
        let mut text = String::new();
        for file in ["lib.rs", "deep.rs", "deep/inner.rs", "win.rs"] {
            let lines = std::fs::read_to_string(src.join(file)).unwrap().lines().count();
            text += &format!("SF:{}\n", src.join(file).display());
            for l in 1..=lines {
                let hits = u64::from(!(file == "lib.rs" && uncovered.contains(&l)));
                text += &format!("DA:{l},{hits}\n");
            }
            text += "end_of_record\n";
        }
        let path = self.dir.join(format!("run-{}.lcov", uncovered.start()));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn crap(&self, src: &Path, lcov: &Path, out: &str, extra: &[&str]) -> Output {
        let baseline = self.dir.join("baseline.json");
        let out = self.dir.join(out);
        assert!(!out.exists(), "each run writes a fresh report");
        Command::new(env!("CARGO_BIN_EXE_quality"))
            .args(["crap", "--platform", "linux", "--revision", REVISION, "--src"])
            .arg(src)
            .arg("--lcov")
            .arg(lcov)
            .arg("--baseline")
            .arg(&baseline)
            .arg("--out")
            .arg(&out)
            .args(extra)
            .output()
            .unwrap()
    }

    fn report(&self, out: &str) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.dir.join(out).join("crap-linux.json")).unwrap()).unwrap()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)
}

/// The run that wrote the baseline, then the same input against it.
fn baselined(run: &Run) -> PathBuf {
    let src = run.source();
    let full = run.lcov(&src, 0..=0);
    let first = run.crap(&src, &full, "first", &["--write-baseline"]);
    assert_eq!(first.status.code(), Some(0), "{}", stdout(&first));
    src
}

#[test]
fn the_measured_baseline_passes_and_its_report_is_tied_to_the_revision() {
    let run = Run::new("baseline");
    let src = baselined(&run);
    let again = run.crap(&src, &run.lcov(&src, 0..=0), "again", &[]);
    assert_eq!(again.status.code(), Some(0), "{}", stdout(&again));
    let report = run.report("again");
    assert_eq!(report["revision"], REVISION);
    assert_eq!(report["platform"], "linux");
    assert_eq!(report["totals"]["functions"], 11);
    assert_eq!(
        report["totals"]["excluded"], 2,
        "win.rs and tests_or_windows aren't built on Linux"
    );
    assert_eq!(report["totals"]["percent"], 100.0);
    assert!(stdout(&again).contains("- Gate: pass"));
}

#[test]
fn removing_coverage_fails_the_gate() {
    let run = Run::new("uncovered");
    let src = baselined(&run);
    // `decide` is lines 10..=25 of lib.rs.
    let out = run.crap(&src, &run.lcov(&src, 10..=25), "uncovered", &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let text = stdout(&out);
    assert!(text.contains("is below the floor 100.0%"), "{text}");
    assert!(
        text.contains("decide (lib.rs:10, complexity 10, coverage 0%, CRAP 110.00): above 30, was 10.00"),
        "{text}"
    );
}

#[test]
fn a_new_function_above_the_crap_limit_fails_the_gate() {
    let run = Run::new("new");
    let src = baselined(&run);
    let lcov = run.lcov(&src, 0..=0);
    // Six decisions and no coverage line: 6² + 6 = 42.
    let mut lib = std::fs::read_to_string(src.join("lib.rs")).unwrap();
    let line = lib.lines().count() + 2;
    lib += "\npub fn added(a: u8) -> u8 {\n    match a { 0 => 0, 1 => 1, 2 => 2, 3 => 3, 4 => 4, _ => 5 }\n}\n";
    std::fs::write(src.join("lib.rs"), lib).unwrap();
    let out = run.crap(&src, &lcov, "new", &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let expected = format!("added (lib.rs:{line}, complexity 6, coverage 0%, CRAP 42.00): new function above 30");
    assert!(stdout(&out).contains(&expected), "{}", stdout(&out));
}

/// `quality crap --report-only` on the branches fixture: real nightly `cargo llvm-cov --branch` output.
fn branch_report(run: &Run, out: &str, lcov: &Path, extra: &[&str]) -> Output {
    let src = run.dir.join("crates/branches/src");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/branches/src"),
        &src,
    );
    Command::new(env!("CARGO_BIN_EXE_quality"))
        .args([
            "crap",
            "--platform",
            "linux",
            "--revision",
            REVISION,
            "--report-only",
            "--src",
        ])
        .arg(&src)
        .arg("--lcov")
        .arg(lcov)
        .arg("--out")
        .arg(run.dir.join(out))
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn an_uncovered_else_shows_as_an_untaken_branch_on_a_covered_line() {
    let run = Run::new("branches");
    let lcov = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/branches/branches.lcov");
    let out = branch_report(&run, "branches", &lcov, &["--require-branches"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let text = stdout(&out);
    assert!(text.contains("- Line coverage: 66.67% (6 of 9 lines)"), "{text}");
    assert!(
        text.contains(
            "- Branch coverage: 50.00% (2 of 4 branches taken, in the functions that ran; \
             1 that never ran have no branch records)"
        ),
        "{text}"
    );
    assert!(text.contains("- Gate: not checked (report only)"), "{text}");
    assert!(text.contains("nightly rustc -Z coverage-options=branch"), "{text}");
    // pick's line 7 ran, yet its else branch never did.
    assert!(text.contains("| 1/2 | 7 | `pick` lib.rs:6 |"), "{text}");
    assert!(text.contains("| 1/2 | 11 | `both` lib.rs:10 |"), "{text}");
    assert!(
        text.contains("| 6.00 | 2 | 0% | not run | `unrun` lib.rs:14 |"),
        "{text}"
    );

    let report = run.report("branches");
    let pick = &report["functions"][0];
    assert_eq!(pick["name"], "pick");
    assert_eq!((pick["lines"].as_u64(), pick["covered"].as_u64()), (Some(3), Some(3)));
    assert_eq!(pick["branches"], serde_json::json!({ "total": 2, "taken": 1 }));
    assert_eq!(pick["untaken_lines"], serde_json::json!([7]));
    assert_eq!(report["functions"][2]["name"], "unrun");
    assert!(report["functions"][2]["branches"].is_null());
    assert_eq!(
        report["totals"]["branches"],
        serde_json::json!({ "total": 4, "taken": 2 })
    );
    assert_eq!(report["totals"]["branch_percent"], 50.0);
}

#[test]
fn the_branch_job_refuses_an_lcov_without_branch_records() {
    let run = Run::new("no-branches");
    let lines_only = run.dir.join("lines-only.lcov");
    let fixture =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/branches/branches.lcov"))
            .unwrap();
    let text: String = fixture
        .lines()
        .filter(|l| !l.starts_with("BR"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&lines_only, text).unwrap();

    let refused = branch_report(&run, "refused", &lines_only, &["--require-branches"]);
    assert_eq!(refused.status.code(), Some(2), "{}", stdout(&refused));
    assert!(stdout(&refused).contains("has no branch records (BRDA)"));

    // Without the switch it reports lines and says branches weren't collected.
    let lines = branch_report(&run, "lines", &lines_only, &[]);
    assert_eq!(lines.status.code(), Some(0), "{}", stdout(&lines));
    assert!(stdout(&lines).contains("- Branch coverage: not collected (no BRDA records in the LCOV)"));
}

#[test]
fn report_only_takes_no_baseline() {
    let run = Run::new("report-only");
    let lcov = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/branches/branches.lcov");
    for extra in [&["--write-baseline"][..], &["--baseline", "b.json"][..]] {
        let out = branch_report(&run, "x", &lcov, extra);
        assert_eq!(out.status.code(), Some(2), "{extra:?}: {}", stdout(&out));
        assert!(stdout(&out).contains("--report-only checks no baseline"));
    }
}

#[test]
fn bad_input_is_a_usage_error_not_a_pass() {
    let run = Run::new("bad");
    let src = run.source();
    let missing = run.crap(&src, &run.dir.join("none.lcov"), "missing", &[]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(stdout(&missing).contains("read "));

    let lcov = run.lcov(&src, 0..=0);
    let no_baseline = run.crap(&src, &lcov, "no-baseline", &[]);
    assert_eq!(no_baseline.status.code(), Some(2), "a missing baseline can't pass");

    std::fs::write(run.dir.join("baseline.json"), "null").unwrap();
    let null = run.crap(&src, &lcov, "null", &[]);
    assert_eq!(
        null.status.code(),
        Some(2),
        "a JSON null baseline can't pass: {}",
        stdout(&null)
    );

    let short = Command::new(env!("CARGO_BIN_EXE_quality"))
        .args(["crap", "--revision", "abc"])
        .output()
        .unwrap();
    assert_eq!(short.status.code(), Some(2));
    assert!(stdout(&short).contains("full commit SHA"));
}
