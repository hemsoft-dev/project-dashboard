//! Line and branch hit counts from an LCOV file, as `cargo llvm-cov --lcov` writes it.

use std::collections::BTreeMap;
use std::fmt;

/// One source file's coverage.
#[derive(Debug, Default)]
pub struct FileCoverage {
    /// Hit counts per instrumented line. A line that isn't instrumented (a blank line, a brace, a comment) has no
    /// entry: it is neither covered nor uncovered.
    pub lines: BTreeMap<usize, u64>,
    /// How often each branch was taken, keyed by its line, block and branch number (`BRDA`). Only a compiler that
    /// collects branch coverage writes these (`cargo llvm-cov --branch`, nightly rustc); otherwise this is empty.
    pub branches: BTreeMap<(usize, u64, u64), u64>,
}

/// Coverage per source file.
#[derive(Debug, Default)]
pub struct Lcov {
    pub files: BTreeMap<String, FileCoverage>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LcovError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for LcovError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lcov line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for LcovError {}

impl Lcov {
    pub fn parse(text: &str) -> Result<Self, LcovError> {
        let mut out = Lcov::default();
        let mut current: Option<String> = None;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim_end();
            let err = |message: &str| LcovError {
                line: i + 1,
                message: message.to_string(),
            };
            if let Some(path) = line.strip_prefix("SF:") {
                if current.is_some() {
                    return Err(err("SF: before the previous file's end_of_record"));
                }
                let path = path.replace('\\', "/");
                out.files.entry(path.clone()).or_default();
                current = Some(path);
            } else if let Some(rest) = line.strip_prefix("DA:") {
                let file = current.as_ref().ok_or_else(|| err("DA: outside a file record"))?;
                let mut parts = rest.split(',');
                let number = parts
                    .next()
                    .and_then(|n| n.parse::<usize>().ok())
                    .ok_or_else(|| err("bad DA line number"))?;
                let hits = parts
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                    .ok_or_else(|| err("bad DA hit count"))?;
                // llvm-cov can list a line twice (two regions); the line ran if either did.
                let entry = out
                    .files
                    .get_mut(file)
                    .expect("file entry exists")
                    .lines
                    .entry(number)
                    .or_insert(0);
                *entry = entry.saturating_add(hits);
            } else if let Some(rest) = line.strip_prefix("BRDA:") {
                let file = current.as_ref().ok_or_else(|| err("BRDA: outside a file record"))?;
                let parts: Vec<&str> = rest.split(',').collect();
                let [number, block, branch, taken] = parts[..] else {
                    return Err(err("BRDA needs line, block, branch and taken"));
                };
                let number = number.parse::<usize>().map_err(|_| err("bad BRDA line number"))?;
                let block = block.parse::<u64>().map_err(|_| err("bad BRDA block"))?;
                let branch = branch.parse::<u64>().map_err(|_| err("bad BRDA branch"))?;
                // `-`: the block holding the branch never ran, so neither did the branch.
                let taken = match taken {
                    "-" => 0,
                    n => n.parse::<u64>().map_err(|_| err("bad BRDA taken count"))?,
                };
                // Repeated like DA lines (one per instantiation): the branch was taken if any instance took it.
                let entry = out
                    .files
                    .get_mut(file)
                    .expect("file entry exists")
                    .branches
                    .entry((number, block, branch))
                    .or_insert(0);
                *entry = entry.saturating_add(taken);
            } else if line == "end_of_record" && current.take().is_none() {
                return Err(err("end_of_record without SF:"));
            }
        }
        if current.is_some() {
            return Err(LcovError {
                line: text.lines().count(),
                message: "file record never ended".to_string(),
            });
        }
        if out.files.is_empty() {
            return Err(LcovError {
                line: 0,
                message: "no file records".to_string(),
            });
        }
        Ok(out)
    }

    /// Whether any file carries branch records: the report was collected with branch coverage.
    pub fn has_branches(&self) -> bool {
        self.files.values().any(|f| !f.branches.is_empty())
    }

    /// The coverage of the file whose path ends with `suffix` (a `/`-separated path relative to the repository).
    /// None when no file matches, or more than one does.
    pub fn file(&self, suffix: &str) -> Option<&FileCoverage> {
        let mut found = self
            .files
            .iter()
            .filter(|(path, _)| path.ends_with(&format!("/{suffix}")) || *path == suffix);
        let first = found.next()?;
        if found.next().is_some() { None } else { Some(first.1) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_per_file_and_merges_repeated_lines() {
        let lcov = Lcov::parse(
            "SF:C:\\repo\\src\\a.rs\nDA:1,0\nDA:2,3\nDA:2,1\nend_of_record\nSF:/repo/src/b.rs\nDA:9,0\nend_of_record\n",
        )
        .unwrap();
        let a = lcov.file("src/a.rs").unwrap();
        assert_eq!(a.lines.get(&1), Some(&0));
        assert_eq!(a.lines.get(&2), Some(&4));
        assert_eq!(lcov.file("src/b.rs").unwrap().lines.len(), 1);
        assert!(lcov.file("src/c.rs").is_none());
        assert!(!lcov.has_branches(), "no BRDA, no branch coverage");
    }

    #[test]
    fn branch_records_count_per_branch_and_merge_repeated_ones() {
        // The shape nightly `cargo llvm-cov --branch` writes: line 2 ran, its else branch never did.
        let lcov = Lcov::parse(
            "SF:/repo/src/a.rs
DA:2,1
BRDA:2,0,0,1
BRDA:2,0,1,0
BRDA:6,0,0,-
BRDA:6,0,0,2
BRDA:6,0,1,-
             BRF:4
BRH:2
end_of_record
",
        )
        .unwrap();
        assert!(lcov.has_branches());
        let a = lcov.file("src/a.rs").unwrap();
        assert_eq!(a.lines.get(&2), Some(&1));
        assert_eq!(
            a.branches.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
            [((2, 0, 0), 1), ((2, 0, 1), 0), ((6, 0, 0), 2), ((6, 0, 1), 0)]
        );
    }

    #[test]
    fn an_ambiguous_suffix_matches_nothing() {
        let lcov = Lcov::parse("SF:/x/src/a.rs\nend_of_record\nSF:/y/src/a.rs\nend_of_record\n").unwrap();
        assert!(lcov.file("src/a.rs").is_none());
        assert!(lcov.file("x/src/a.rs").is_some());
    }

    #[test]
    fn malformed_records_are_errors_not_empty_coverage() {
        for (text, message) in [
            ("", "no file records"),
            ("DA:1,1\n", "DA: outside a file record"),
            ("SF:a.rs\nDA:x,1\nend_of_record\n", "bad DA line number"),
            ("SF:a.rs\nDA:1\nend_of_record\n", "bad DA hit count"),
            ("SF:a.rs\nDA:1,1\n", "file record never ended"),
            ("SF:a.rs\nSF:b.rs\n", "SF: before the previous file's end_of_record"),
            ("end_of_record\n", "end_of_record without SF:"),
        ] {
            let err = Lcov::parse(text).unwrap_err();
            assert_eq!(err.message, message, "{text:?}");
        }
    }
}
