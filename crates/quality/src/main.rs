//! Quality gates for dashboard-core. See docs/quality.md.
//!
//! ```text
//! quality crap --src crates/dashboard-core/src --lcov core.lcov --platform linux --revision <sha> \
//!              --baseline quality/crap-baseline-linux.json --out target/quality [--write-baseline]
//! ```
//!
//! Exits 0 when the gate passes, 1 when it fails, 2 on a usage or input error.

mod crap;
mod lcov;
mod source;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::crap::Baseline;
use crate::lcov::Lcov;
use crate::source::Platform;

const USAGE: &str = "usage: quality crap --src DIR --lcov FILE --platform linux|windows --revision SHA \
--baseline FILE --out DIR [--write-baseline]";

struct Args {
    src: PathBuf,
    lcov: PathBuf,
    platform: Platform,
    revision: String,
    baseline: PathBuf,
    out: PathBuf,
    write_baseline: bool,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    if args.next().as_deref() != Some("crap") {
        return Err(USAGE.to_string());
    }
    let (mut src, mut lcov, mut platform, mut revision, mut baseline, mut out) = (None, None, None, None, None, None);
    let mut write_baseline = false;
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--src" => src = Some(PathBuf::from(value()?)),
            "--lcov" => lcov = Some(PathBuf::from(value()?)),
            "--platform" => {
                let v = value()?;
                platform = Some(Platform::parse(&v).ok_or_else(|| format!("unknown platform {v:?}"))?);
            }
            "--revision" => {
                let v = value()?;
                if v.len() != 40 || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(format!("--revision must be a full commit SHA, not {v:?}"));
                }
                revision = Some(v);
            }
            "--baseline" => baseline = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--write-baseline" => write_baseline = true,
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    fn need<T>(v: Option<T>, name: &str) -> Result<T, String> {
        v.ok_or_else(|| format!("{name} is required\n{USAGE}"))
    }
    Ok(Args {
        src: need(src, "--src")?,
        lcov: need(lcov, "--lcov")?,
        platform: need(platform, "--platform")?,
        revision: need(revision, "--revision")?,
        baseline: need(baseline, "--baseline")?,
        out: need(out, "--out")?,
        write_baseline,
    })
}

fn run(args: &Args) -> Result<bool, String> {
    let functions = source::functions(&args.src, args.platform).map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&args.lcov).map_err(|e| format!("read {}: {e}", args.lcov.display()))?;
    let lcov = Lcov::parse(&text).map_err(|e| format!("{}: {e}", args.lcov.display()))?;
    let prefix = repo_relative(&args.src);
    let report = crap::join(&functions, &lcov, &prefix, args.platform, &args.revision).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&args.out).map_err(|e| format!("create {}: {e}", args.out.display()))?;
    let stem = format!("crap-{}", args.platform.name());
    write(&args.out.join(format!("{stem}.json")), &json(&report)?)?;
    if args.write_baseline {
        let baseline = Baseline::from_report(&report);
        write(&args.baseline, &json(&baseline)?)?;
        println!(
            "wrote {} (floor {:.1}%)",
            args.baseline.display(),
            baseline.line_coverage_floor
        );
    }
    let baseline: Baseline = {
        let text =
            std::fs::read_to_string(&args.baseline).map_err(|e| format!("read {}: {e}", args.baseline.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", args.baseline.display()))?
    };
    let verdict = crap::check(&report, &baseline);
    let md = crap::markdown(&report, &verdict, 25);
    write(&args.out.join(format!("{stem}.md")), &md)?;
    print!("{md}");
    Ok(verdict.passed())
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|s| s + "\n")
        .map_err(|e| e.to_string())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

/// `crates/dashboard-core/src` from any spelling of that folder: LCOV paths are absolute, so the join matches on this
/// suffix.
fn repo_relative(src: &Path) -> String {
    let text = src.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let from = parts.iter().rposition(|p| *p == "crates").unwrap_or(0);
    parts[from..].join("/")
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("quality: {e}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("quality: {e}");
            ExitCode::from(2)
        }
    }
}
