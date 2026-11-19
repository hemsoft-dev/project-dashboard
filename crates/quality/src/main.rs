//! Quality gates for dashboard-core. See docs/quality.md.
//!
//! ```text
//! quality crap --src crates/dashboard-core/src --lcov core.lcov --platform linux --revision <sha> \
//!              --baseline crates/quality/baselines/crap-linux.json --out target/quality [--write-baseline]
//! quality crap --src crates/dashboard-core/src --lcov core-branches.lcov --platform linux --revision <sha> \
//!              --report-only [--require-branches] --out target/branch-quality
//! quality mutants --src crates/dashboard-core/src --outcomes mutants.out/outcomes.json --mode full|diff \
//!                 --revision <sha> --policy crates/quality/baselines/mutants-linux.json --out target/quality
//! ```
//!
//! Exits 0 when the gate passes, 1 when it fails, 2 on a usage or input error. `--report-only` checks no baseline
//! (the branch-coverage job, #110) and exits 0 once the report is written.

mod crap;
mod lcov;
mod mutants;
mod source;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::crap::Baseline;
use crate::lcov::Lcov;
use crate::mutants::{Mode, Policy};
use crate::source::Platform;

const USAGE: &str = "usage: quality crap --src DIR --lcov FILE --platform linux|windows --revision SHA \
--baseline FILE --out DIR [--write-baseline]
       quality crap --src DIR --lcov FILE --platform linux|windows --revision SHA --report-only \
[--require-branches] --out DIR
       quality mutants --src DIR --outcomes FILE --mode full|diff --revision SHA --policy FILE --out DIR";

/// `--name value` pairs and bare `--switch`es after the subcommand.
struct Flags {
    values: BTreeMap<String, String>,
    switches: Vec<String>,
}

impl Flags {
    fn parse(args: impl Iterator<Item = String>, switches: &[&str], values: &[&str]) -> Result<Self, String> {
        let mut flags = Self {
            values: BTreeMap::new(),
            switches: Vec::new(),
        };
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            if switches.contains(&flag.as_str()) {
                flags.switches.push(flag);
            } else if values.contains(&flag.as_str()) {
                let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
                flags.values.insert(flag, value);
            } else {
                return Err(format!("unknown argument {flag:?}\n{USAGE}"));
            }
        }
        Ok(flags)
    }

    fn need(&self, name: &str) -> Result<&str, String> {
        self.values
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("{name} is required\n{USAGE}"))
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        self.need(name).map(PathBuf::from)
    }

    fn revision(&self) -> Result<String, String> {
        let v = self.need("--revision")?;
        if v.len() != 40 || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("--revision must be a full commit SHA, not {v:?}"));
        }
        Ok(v.to_string())
    }

    fn has(&self, switch: &str) -> bool {
        self.switches.iter().any(|s| s == switch)
    }
}

fn platform(flags: &Flags) -> Result<Platform, String> {
    let v = flags.need("--platform")?;
    Platform::parse(v).ok_or_else(|| format!("unknown platform {v:?}"))
}

fn crap_command(flags: &Flags) -> Result<bool, String> {
    // A malformed revision is reported first, before any missing flag, as the old parser did.
    let revision = flags.revision()?;
    let (src, lcov_path, platform) = (flags.path("--src")?, flags.path("--lcov")?, platform(flags)?);
    let report_only = flags.has("--report-only");
    if report_only && (flags.has("--write-baseline") || flags.values.contains_key("--baseline")) {
        return Err(format!("--report-only checks no baseline and writes none\n{USAGE}"));
    }
    let baseline_path = if report_only {
        None
    } else {
        Some(flags.path("--baseline")?)
    };
    let out = flags.path("--out")?;
    let functions = source::functions(&src, platform).map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&lcov_path).map_err(|e| format!("read {}: {e}", lcov_path.display()))?;
    let lcov = Lcov::parse(&text).map_err(|e| format!("{}: {e}", lcov_path.display()))?;
    // The branch-coverage job must not report an empty branch column as if it had measured one.
    if flags.has("--require-branches") && !lcov.has_branches() {
        return Err(format!(
            "{} has no branch records (BRDA); collect it with `cargo llvm-cov --branch` on nightly rustc",
            lcov_path.display()
        ));
    }
    let report = crap::join(&functions, &lcov, &repo_relative(&src), platform, &revision).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let stem = format!("crap-{}", platform.name());
    write(&out.join(format!("{stem}.json")), &json(&report)?)?;
    let Some(baseline_path) = baseline_path else {
        let md = crap::markdown(&report, None, 25);
        write(&out.join(format!("{stem}.md")), &md)?;
        print!("{md}");
        return Ok(true);
    };
    if flags.has("--write-baseline") {
        let baseline = Baseline::from_report(&report);
        write(&baseline_path, &json(&baseline)?)?;
        println!(
            "wrote {} (floor {:.1}%)",
            baseline_path.display(),
            baseline.line_coverage_floor
        );
    }
    let baseline: Baseline = read_json(&baseline_path)?;
    let verdict = crap::check(&report, &baseline);
    let md = crap::markdown(&report, Some(&verdict), 25);
    write(&out.join(format!("{stem}.md")), &md)?;
    print!("{md}");
    Ok(verdict.passed())
}

fn mutants_command(flags: &Flags) -> Result<bool, String> {
    let revision = flags.revision()?;
    let (src, outcomes_path, out) = (flags.path("--src")?, flags.path("--outcomes")?, flags.path("--out")?);
    let mode = flags.need("--mode")?;
    let mode = Mode::parse(mode).ok_or_else(|| format!("unknown mode {mode:?}"))?;
    let policy: Policy = read_json(&flags.path("--policy")?)?;
    let functions = source::functions(&src, policy.platform).map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&outcomes_path).map_err(|e| format!("read {}: {e}", outcomes_path.display()))?;
    // The plan sits beside the outcomes in cargo-mutants' mutants.out; without it completeness can't be shown.
    let plan_path = outcomes_path.with_file_name("mutants.json");
    let plan = std::fs::read_to_string(&plan_path).map_err(|e| format!("read {}: {e}", plan_path.display()))?;
    let report = mutants::read(&text, &plan, &functions, &repo_relative(&src), &policy, mode, &revision)
        .map_err(|e| e.to_string())?;
    let failures = mutants::check(&report, &policy);
    std::fs::create_dir_all(&out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let stem = format!("mutants-{}-{}", mode.name(), policy.platform.name());
    write(&out.join(format!("{stem}.json")), &json(&report)?)?;
    let md = mutants::markdown(&report, &policy, &failures);
    write(&out.join(format!("{stem}.md")), &md)?;
    print!("{md}");
    Ok(failures.is_empty())
}

fn run(mut args: impl Iterator<Item = String>) -> Result<bool, String> {
    match args.next().as_deref() {
        Some("crap") => crap_command(&Flags::parse(
            args,
            &["--write-baseline", "--report-only", "--require-branches"],
            &["--src", "--lcov", "--platform", "--revision", "--baseline", "--out"],
        )?),
        Some("mutants") => mutants_command(&Flags::parse(
            args,
            &[],
            &["--src", "--outcomes", "--mode", "--revision", "--policy", "--out"],
        )?),
        _ => Err(USAGE.to_string()),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|s| s + "\n")
        .map_err(|e| e.to_string())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

/// `crates/dashboard-core/src` from any spelling of that folder: LCOV and cargo-mutants paths are relative to the
/// repository or absolute, so reports match on this suffix.
fn repo_relative(src: &Path) -> String {
    let text = src.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let from = parts.iter().rposition(|p| *p == "crates").unwrap_or(0);
    parts[from..].join("/")
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("quality: {e}");
            ExitCode::from(2)
        }
    }
}
