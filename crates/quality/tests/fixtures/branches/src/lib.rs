//! Branch-coverage fixture (#110). `branches.lcov` beside this folder is real `cargo llvm-cov --branch` output for
//! this file, from nightly-2026-10-22, with its SF path replaced. The test takes only the `if` arm and only `a`
//! false, so each function's line runs while one of its branches never does. `unrun` never runs, and LLVM writes no
//! branch record for it.

pub fn pick(a: bool) -> u8 {
    if a { 1 } else { 2 }
}

pub fn both(a: bool, b: bool) -> bool {
    a && b
}

pub fn unrun(a: bool) -> u8 {
    if a { 3 } else { 4 }
}

#[test]
fn only_true() {
    assert_eq!(pick(true), 1);
    assert!(!both(false, true));
}
