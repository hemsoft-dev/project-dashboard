//! Fleet observations are independent of registration and project status.
use crate::{pmd::ReadApi, priority::Pmd, source::FetchError};
use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub name: String,
    pub online: Option<bool>,
    pub update_available: Option<bool>,
    pub channel: Option<String>,
    pub probe_ok: Option<bool>,
    pub server_version: Option<String>,
    pub client_version: Option<String>,
    pub latest_version: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub attempted_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub name: String,
    pub os: String,
    pub max_sessions: u64,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Load {
    pub name: String,
    pub used: Option<u64>,
    pub chief_started: Option<u64>,
    pub free: Option<u64>,
    pub load_unknown: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub machines: Vec<Machine>,
    pub observations: Vec<Observation>,
    pub loads: Vec<Load>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub assignments: Vec<crate::backlog::Assignment>,
    pub truncated: bool,
}
impl Snapshot {
    pub fn fetch(pmd: &Pmd) -> Result<Self, FetchError> {
        let api = ReadApi::new(pmd, std::time::Duration::from_secs(3))?;
        #[derive(Deserialize)]
        struct Machines {
            machines: Vec<Machine>,
        }
        #[derive(Deserialize)]
        struct Capacity {
            machines: Vec<Load>,
        }
        #[derive(Deserialize)]
        struct Health {
            schema: u64,
            machines: Vec<Observation>,
        }
        #[derive(Deserialize)]
        struct Assignments {
            assignments: Vec<crate::backlog::Assignment>,
            truncated: bool,
        }
        let work: Assignments = api.get("v1/assignments", &[("limit", "500")])?;
        let machines: Machines = api.get("v1/machines", &[])?;
        let capacity: Capacity = api.get("v1/capacity", &[])?;
        let health: Health = api.get("v1/fleet/status", &[])?;
        if health.schema != 1 {
            return Err(FetchError::UnsupportedSchema);
        }
        Ok(Self {
            machines: machines.machines,
            observations: health.machines,
            loads: capacity.machines,
            fetched_at: Some(Utc::now()),
            assignments: work.assignments,
            truncated: work.truncated,
        })
    }
}
impl Observation {
    pub fn stale(&self, now: DateTime<Utc>) -> bool {
        self.attempted_at.is_none_or(|at| {
            now.signed_duration_since(at).num_seconds() > 600 || at > now + chrono::Duration::minutes(1)
        })
    }
    pub fn connectivity(&self, now: DateTime<Utc>) -> &'static str {
        if self.stale(now) {
            "Stale"
        } else {
            match self.online {
                Some(true) => "Online",
                Some(false) => "Offline",
                None => "Unknown",
            }
        }
    }
    pub fn server_status(&self, now: DateTime<Utc>) -> &'static str {
        if self.stale(now) {
            return "Stale";
        }
        if self.probe_ok != Some(true) {
            return "Unknown · probe failed";
        }
        if self
            .checked_at
            .is_none_or(|at| now.signed_duration_since(at).num_seconds() > 600)
        {
            return "Stale";
        }
        match (&self.server_version, &self.latest_version) {
            (Some(a), Some(b)) if a == b => "Current",
            (Some(_), Some(_)) => {
                if self.update_available == Some(true) {
                    "Update available"
                } else {
                    "Version differs"
                }
            }
            (Some(_), None) => "Latest unknown",
            _ => "Not responding",
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_does_not_imply_online() {
        let now = Utc::now();
        let mut o = Observation {
            attempted_at: Some(now),
            ..Default::default()
        };
        assert_eq!(o.connectivity(now), "Unknown");
        o.online = Some(false);
        assert_eq!(o.connectivity(now), "Offline");
        o.online = Some(true);
        o.probe_ok = Some(false);
        assert_eq!(o.server_status(now), "Unknown · probe failed");
        o.probe_ok = Some(true);
        o.checked_at = Some(now);
        o.server_version = Some("1".into());
        o.latest_version = Some("1".into());
        assert_eq!(o.server_status(now), "Current");
        assert_eq!(o.server_status(now + chrono::Duration::minutes(11)), "Stale");
    }
}

#[cfg(test)]
mod api_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    #[test]
    fn fetch_joins_only_hosted_data_and_keeps_unknowns() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for _ in 0..4 {
                let (mut stream, _) = server.accept().unwrap();
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                let body = if request.contains("/v1/machines ") {
                    r#"{"machines":[{"name":"air","os":"macos","maxSessions":2}]}"#
                } else if request.contains("/v1/capacity ") {
                    r#"{"machines":[{"name":"air","used":null,"free":null,"loadUnknown":"stale"}]}"#
                } else if request.contains("/v1/assignments?") {
                    r#"{"assignments":[],"truncated":false}"#
                } else {
                    r#"{"schema":1,"machines":[]}"#
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let dir = std::env::temp_dir().join(format!(
            "fleet-api-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let token = dir.join("token");
        std::fs::write(&token, "fixture-token").unwrap();
        let data = Snapshot::fetch(&Pmd { url, token_path: token }).unwrap();
        worker.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(data.machines[0].name, "air");
        assert!(data.observations.is_empty());
        assert_eq!(data.loads[0].used, None);
    }
}
