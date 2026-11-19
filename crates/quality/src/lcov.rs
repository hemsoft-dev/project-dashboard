//! Line hit counts from an LCOV file, as `cargo llvm-cov --lcov` writes it.

use std::collections::BTreeMap;
use std::fmt;

/// Hit counts per instrumented line, per source file. A line that isn't instrumented (a blank line, a brace, a
/// comment) has no entry: it is neither covered nor uncovered.
#[derive(Debug, Default)]
pub struct Lcov {
    pub files: BTreeMap<String, BTreeMap<usize, u64>>,
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
                    .entry(number)
                    .or_insert(0);
                *entry = entry.saturating_add(hits);
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

    /// The hit counts for the file whose path ends with `suffix` (a `/`-separated path relative to the repository).
    /// None when no file matches, or more than one does.
    pub fn file(&self, suffix: &str) -> Option<&BTreeMap<usize, u64>> {
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
        assert_eq!(a.get(&1), Some(&0));
        assert_eq!(a.get(&2), Some(&4));
        assert_eq!(lcov.file("src/b.rs").unwrap().len(), 1);
        assert!(lcov.file("src/c.rs").is_none());
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
