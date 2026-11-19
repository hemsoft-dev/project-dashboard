//! A fixture crate for the quality tool's tests. It is never compiled.

pub mod deep;
#[cfg(windows)]
mod win;
// No such file: a test-only module is never read.
#[cfg(test)]
mod tests_only;

pub fn decide(a: bool, b: bool, n: Option<u8>) -> Option<u8> {
    if a && b {
        return Some(0);
    }
    let v = n?;
    let Some(w) = v.checked_add(1) else { return None };
    let small = |x: u8| x < 2 || x == 7;
    match w {
        0 => Some(1),
        x if x > 9 => Some(2),
        _ => {
            for _ in 0..2 {}
            Some(u8::from(small(w)))
        }
    }
}

pub fn outer() -> u8 {
    fn helper(x: u8) -> u8 {
        if x > 0 { x } else { 1 }
    }
    helper(2)
}

pub struct S;

impl S {
    pub fn m(&self) -> u8 {
        1
    }

    #[cfg(test)]
    fn only_in_tests(&self) {}
}

impl std::fmt::Display for S {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "s")
    }
}

#[cfg(target_os = "linux")]
pub fn linux_only() {}

#[cfg(any(test, windows))]
pub fn tests_or_windows() {}

#[cfg(all(test, unix))]
fn unix_tests() {}

#[test]
fn a_test() {}
