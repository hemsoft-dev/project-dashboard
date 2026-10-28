//! Transport recovery is independent of the configured periodic refresh.
use crate::{source::FetchError, status::StatusDocument};
use chrono::{DateTime, Local};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recovery {
    pub last_success: Option<DateTime<Local>>,
    pub retry_at: Option<Instant>,
    failures: usize,
}
impl Recovery {
    pub fn record(&mut self, result: &Result<StatusDocument, FetchError>, now: Instant, wall: DateTime<Local>) {
        let transport_failure = match result {
            Err(error) => error.is_transient(),
            Ok(doc) => doc
                .hosted
                .as_ref()
                .is_some_and(|hosted| hosted.work_transport_error.is_some()),
        };
        if transport_failure {
            let seconds = [5, 10, 20, 60][self.failures.min(3)];
            self.failures = self.failures.saturating_add(1);
            self.retry_at = Some(now + Duration::from_secs(seconds));
        } else {
            if result.is_ok() {
                self.last_success = Some(wall);
            }
            self.failures = 0;
            self.retry_at = None;
        }
    }
    pub fn due(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|at| now >= at)
    }
}
