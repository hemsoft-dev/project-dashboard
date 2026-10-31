//! Runtime observations are independent of dispatch allocation and version probes.
use crate::{pmd::ReadApi, priority::Pmd, source::FetchError};
use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub environment: String,
    pub thread_id: String,
    pub title: String,
    pub project: String,
    pub runtime_status: String,
    pub role: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub name: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub state: String,
    pub reason: Option<String>,
    pub active_count: Option<u64>,
    pub chief_active: bool,
    pub sessions: Vec<Session>,
}

impl Machine {
    pub fn current(&self, now: DateTime<Utc>) -> bool {
        matches!(self.state.as_str(), "ok" | "partial")
            && self.observed_at.is_some_and(|at| {
                let age = now.signed_duration_since(at).num_seconds();
                (-15..=90).contains(&age)
            })
    }

    pub fn count_label(&self, now: DateTime<Utc>) -> String {
        if !self.current(now) {
            return if self
                .observed_at
                .is_some_and(|at| now.signed_duration_since(at).num_seconds() > 90)
            {
                "Stale".into()
            } else {
                "Unavailable".into()
            };
        }
        match self.active_count {
            Some(count) if self.state == "partial" => format!("≥{count} active · partial"),
            Some(count) => format!("{count} active"),
            None => "Unavailable".into(),
        }
    }

    pub fn work_label(&self, now: DateTime<Utc>, failed_refresh: bool) -> String {
        let current = self.current(now) && !failed_refresh;
        if self.sessions.is_empty() {
            return if current && self.state == "ok" && self.active_count == Some(0) {
                "None active".into()
            } else {
                self.reason
                    .clone()
                    .unwrap_or_else(|| "Runtime activity unavailable".into())
            };
        }
        self.sessions
            .iter()
            .map(|session| {
                let title = if session.title.is_empty() {
                    "Untitled session"
                } else {
                    &session.title
                };
                format!(
                    "{}{} · {}",
                    if current { "" } else { "Last known · " },
                    session.runtime_status,
                    title
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema: u64,
    pub observed_at: Option<DateTime<Utc>>,
    pub machines: Vec<Machine>,
}

impl Snapshot {
    pub fn fetch(pmd: &Pmd) -> Result<Self, FetchError> {
        let api = ReadApi::new(pmd, std::time::Duration::from_secs(3))?;
        let result: Self = api.get("v1/fleet/activity", &[])?;
        if result.schema != 1 {
            return Err(FetchError::UnsupportedSchema);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitoring_partial_and_unavailable_never_become_idle_zero() {
        let now = Utc::now();
        let mut machine = Machine {
            observed_at: Some(now),
            state: "partial".into(),
            active_count: Some(1),
            sessions: vec![Session {
                title: "hs-buddy #823".into(),
                runtime_status: "monitoring".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(machine.count_label(now), "≥1 active · partial");
        assert!(machine.work_label(now, false).contains("monitoring · hs-buddy #823"));
        machine.state = "unavailable".into();
        machine.active_count = None;
        assert_eq!(machine.count_label(now), "Unavailable");
        assert!(machine.work_label(now, false).starts_with("Last known"));
        machine.state = "ok".into();
        machine.active_count = Some(0);
        machine.sessions.clear();
        assert_eq!(machine.work_label(now, false), "None active");
        assert_eq!(machine.count_label(now + chrono::Duration::seconds(91)), "Stale");
    }
}
