//! The app starts no child process, so it never depends on the PATH it inherits (#38). v0.4.0 ran `gh x status` in
//! every checkout; launched from a Windows Explorer with a stale PATH, gh-x couldn't find git and every row failed.
use std::path::{Path, PathBuf};

/// Ways to start a process from Rust's standard library.
const FORBIDDEN: [&str; 4] = ["Command::new", "process::Command", "CommandExt", "Stdio"];

fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn no_production_source_starts_a_child_process() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for krate in ["dashboard-core", "dashboard-app"] {
        rust_files(&crates.join(krate).join("src"), &mut files);
    }
    // A wrong path would make the check pass vacuously.
    for expected in ["dashboard-core/src/pmd.rs", "dashboard-app/src/main.rs"] {
        assert!(
            files.iter().any(|file| file.ends_with(expected)),
            "{expected} wasn't scanned"
        );
    }
    let offenders: Vec<String> = files
        .iter()
        .flat_map(|file| {
            let source = std::fs::read_to_string(file).unwrap();
            FORBIDDEN
                .iter()
                .filter(|pattern| source.contains(**pattern))
                .map(|pattern| format!("{}: {pattern}", file.display()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "production code must not start child processes:\n{}",
        offenders.join("\n")
    );
}
