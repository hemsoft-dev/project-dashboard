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
