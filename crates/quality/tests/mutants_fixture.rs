//! The mutation gate end to end with the real cargo-mutants (#101): a tiny crate whose test asserts passes, and the
//! same crate with the assertion removed leaves every mutant missed and fails the break threshold.
//!
//! Ignored by default because it needs cargo-mutants 27.1.0 and builds the crate once per mutant:
//! `cargo test -p quality --test mutants_fixture -- --ignored`. The mutation workflow runs it.

use std::path::{Path, PathBuf};
use std::process::Command;

const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

fn platform() -> &'static str {
    if cfg!(windows) { "windows" } else { "linux" }
}

/// A standalone crate (its own workspace) whose one test either asserts or only calls.
fn fixture(dir: &Path, test_body: &str) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"mutants-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        format!(
            "pub fn add(a: i32, b: i32) -> i32 {{\n    a + b\n}}\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn adds() {{\n        {test_body}\n    }}\n}}\n"
        ),
    )
    .unwrap();
    let policy = serde_json::json!({
        "platform": platform(),
        "files": ["src/lib.rs"],
        "break_score": 100.0,
        "target_score": 100.0,
        "min_mutants": 1,
        "baseline": {"revision": REVISION, "counts": {
            "total": 0, "caught": 0, "missed": 0, "timeout": 0, "unviable": 0, "excluded": 0, "denominator": 0, "score": 0.0
        }},
        "equivalent": [],
    });
    std::fs::write(dir.join("policy.json"), policy.to_string()).unwrap();
}

struct Gate {
    code: Option<i32>,
    report: serde_json::Value,
    output: String,
}

fn mutate_and_check(dir: &Path) -> Gate {
    let out = dir.join("out");
    assert!(!out.exists(), "a fresh run");
    let mutants = Command::new("cargo")
        .args(["mutants", "--no-shuffle", "--jobs", "1", "--dir"])
        .arg(dir)
        .arg("--output")
        .arg(&out)
        .output()
        .expect("cargo-mutants is installed");
    let outcomes = out.join("mutants.out/outcomes.json");
    assert!(
        outcomes.is_file(),
        "cargo mutants wrote no outcomes: {}",
        String::from_utf8_lossy(&mutants.stderr)
    );
    let report_dir = dir.join("report");
    let gate = Command::new(env!("CARGO_BIN_EXE_quality"))
        .args(["mutants", "--mode", "full", "--revision", REVISION, "--src"])
        .arg(dir.join("src"))
        .arg("--outcomes")
        .arg(&outcomes)
        .arg("--policy")
        .arg(dir.join("policy.json"))
        .arg("--out")
        .arg(&report_dir)
        .output()
        .unwrap();
    let report = std::fs::read_to_string(report_dir.join(format!("mutants-full-{}.json", platform()))).unwrap();
    Gate {
        code: gate.status.code(),
        report: serde_json::from_str(&report).unwrap(),
        output: String::from_utf8_lossy(&gate.stdout).into_owned() + &String::from_utf8_lossy(&gate.stderr),
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("quality-mutants-{}-{name}", std::process::id()))
}

#[test]
#[ignore = "needs cargo-mutants 27.1.0; run with --ignored"]
fn an_asserting_test_catches_every_mutant_and_the_gate_passes() {
    let dir = scratch("asserts");
    fixture(&dir, "assert_eq!(add(2, 3), 5);");
    let gate = mutate_and_check(&dir);
    assert_eq!(gate.code, Some(0), "{}", gate.output);
    let counts = &gate.report["counts"];
    assert!(counts["total"].as_u64().unwrap() >= 3, "{counts}");
    assert_eq!(counts["missed"], 0);
    assert_eq!(counts["timeout"], 0);
    assert_eq!(counts["caught"], counts["denominator"]);
    assert_eq!(gate.report["revision"], REVISION);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs cargo-mutants 27.1.0; run with --ignored"]
fn removing_the_assertion_leaves_missed_mutants_and_fails_the_break_threshold() {
    let dir = scratch("no-assert");
    fixture(&dir, "let _ = add(2, 3);");
    let gate = mutate_and_check(&dir);
    assert_eq!(gate.code, Some(1), "{}", gate.output);
    let counts = &gate.report["counts"];
    assert!(counts["missed"].as_u64().unwrap() >= 3, "{counts}");
    assert_eq!(counts["caught"], 0);
    assert!(
        gate.output.contains("is below the break threshold 100.0%"),
        "{}",
        gate.output
    );
    let _ = std::fs::remove_dir_all(&dir);
}
