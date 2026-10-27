//! The Fleet dialog's data, without a window (#36): pmd's machines, subscriptions and agents, and the full-row `PUT`
//! that creates or replaces one.
//!
//! Each kind of row is an [`edit::Form`](crate::edit::Form) over its fields as text; lists such as capabilities are
//! comma-separated words. [`body`] checks the text the way pmd does ([Fleet config](
//! https://github.com/hemsoft-dev/project-manager#fleet-config)) and builds the row pmd wants: every field, since a
//! `PUT` replaces the row. There is no delete: an agent is disabled and a machine gets `maxSessions` 0. Saving a row
//! whose `source` is `config` makes it an `api` row, which the config file no longer updates.
use std::collections::BTreeMap;
use std::net::IpAddr;

use serde_json::{Value, json};

use crate::edit::Row;
use crate::work::FieldProblem;
use crate::write::WriteError;

/// pmd's limits.
pub const MAX_SESSIONS: u64 = 64;
pub const MAX_TAGS: usize = 32;
pub const MAX_PLAN: usize = 100;
/// The words in use now, offered as suggestions; pmd accepts any lower-case word with `_`.
pub const CAPABILITIES: [&str; 5] = ["linux", "windows", "apple", "browser", "computer_use"];
pub const ROLES: [&str; 3] = ["implement", "review", "research"];
pub const OSES: [&str; 3] = ["linux", "windows", "macos"];

/// The three kinds of fleet row, one tab each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Machine,
    Subscription,
    Agent,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Machine, Kind::Subscription, Kind::Agent];

    /// The tab's title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Machine => "Machines",
            Self::Subscription => "Subscriptions",
            Self::Agent => "Agents",
        }
    }

    /// One row's name, as in "Add machine".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::Subscription => "subscription",
            Self::Agent => "agent",
        }
    }

    /// `GET` of the list, and the key the list sits under in pmd's answer.
    pub fn list_path(self) -> &'static str {
        match self {
            Self::Machine => "v1/machines",
            Self::Subscription => "v1/subscriptions",
            Self::Agent => "v1/agents",
        }
    }

    fn list_key(self) -> &'static str {
        &self.list_path()[3..]
    }

    /// The API path of one row. An agent's id has slashes, which pmd's routes take as they are.
    pub fn path(self, key: &str) -> String {
        format!("{}/{key}", self.list_path())
    }

    /// The editor's fields, as pmd names them, so a `400` naming one lands next to it.
    pub fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Machine => &["name", "os", "tailnetIp", "maxSessions", "capabilities"],
            Self::Subscription => &["id", "provider", "plan", "metered"],
            Self::Agent => &[
                "machine",
                "provider",
                "alias",
                "model",
                "subscription",
                "roles",
                "enabled",
            ],
        }
    }

    /// The fields that make up the row's key: set when creating, read-only afterwards.
    pub fn key_fields(self) -> &'static [&'static str] {
        match self {
            Self::Machine => &["name"],
            Self::Subscription => &["id"],
            Self::Agent => &["machine", "provider", "alias"],
        }
    }

    /// Where `409 already_exists` belongs.
    pub fn key_field(self) -> &'static str {
        match self {
            Self::Machine => "name",
            Self::Subscription => "id",
            Self::Agent => "alias",
        }
    }

    pub fn label(self, field: &str) -> &'static str {
        match (self, field) {
            (Self::Machine, "name") => "Name",
            (_, "os") => "OS",
            (_, "tailnetIp") => "Tailnet IP",
            (_, "maxSessions") => "Max sessions",
            (_, "capabilities") => "Capabilities",
            (Self::Subscription, "id") => "Id",
            (_, "provider") => "Provider",
            (_, "plan") => "Plan",
            (_, "metered") => "Metered",
            (_, "machine") => "Machine",
            (_, "alias") => "Alias",
            (_, "model") => "Model",
            (_, "subscription") => "Subscription",
            (_, "roles") => "Roles",
            (_, "enabled") => "Enabled",
            _ => "Field",
        }
    }

    /// What a new row starts with.
    pub fn defaults(self) -> BTreeMap<String, String> {
        let pairs: &[(&str, &str)] = match self {
            Self::Machine => &[("os", "linux"), ("maxSessions", "1")],
            Self::Subscription => &[("metered", "false")],
            Self::Agent => &[("enabled", "true")],
        };
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }
}

/// One stored row as the dialog shows it: its key, who owns it, its version and its fields as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetRow {
    pub kind: Kind,
    pub key: String,
    /// `config` while the config file owns it, `api` after a `PUT` changed it.
    pub source: String,
    pub version: u64,
    pub values: BTreeMap<String, String>,
}

impl FleetRow {
    pub fn from_json(kind: Kind, value: &Value) -> Result<Self, WriteError> {
        let text = |field: &str| match value.get(field) {
            Some(Value::String(text)) => Ok(text.clone()),
            Some(Value::Null) => Ok(String::new()),
            Some(Value::Number(number)) => Ok(number.to_string()),
            Some(Value::Bool(flag)) => Ok(flag.to_string()),
            Some(Value::Array(words)) => Ok(words.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")),
            _ => Err(WriteError::Malformed),
        };
        let key_name = match kind {
            Kind::Machine => "name",
            Kind::Subscription | Kind::Agent => "id",
        };
        let key = text(key_name)?;
        let mut values = BTreeMap::new();
        for field in kind.fields() {
            let value = match (kind, *field) {
                (Kind::Agent, "alias") => key.splitn(3, '/').nth(2).ok_or(WriteError::Malformed)?.to_owned(),
                _ => text(field)?,
            };
            values.insert((*field).to_owned(), value);
        }
        Ok(Self {
            kind,
            key,
            source: value
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            version: value
                .get("version")
                .and_then(Value::as_u64)
                .ok_or(WriteError::Malformed)?,
            values,
        })
    }

    /// A list from `GET /v1/machines`, `/v1/subscriptions` or `/v1/agents`.
    pub fn list_from_json(kind: Kind, value: &Value) -> Result<Vec<Self>, WriteError> {
        value
            .get(kind.list_key())
            .and_then(Value::as_array)
            .ok_or(WriteError::Malformed)?
            .iter()
            .map(|row| Self::from_json(kind, row))
            .collect()
    }

    /// The editor's starting values.
    pub fn row(&self) -> Row {
        Row {
            version: self.version,
            values: self.values.clone(),
        }
    }

    pub fn path(&self) -> String {
        self.kind.path(&self.key)
    }

    pub fn value(&self, field: &str) -> &str {
        self.values.get(field).map_or("", String::as_str)
    }

    pub fn from_config(&self) -> bool {
        self.source == "config"
    }
}

/// What a check needs to know about the rest of the fleet: the machine names and subscription ids that exist, so an
/// agent naming another is caught before pmd's `400 unknown_machine` or `unknown_subscription`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Known {
    pub machines: Vec<String>,
    pub subscriptions: Vec<String>,
}

fn is_slug(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && text.len() <= 63
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_provider(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && text.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_model(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && text.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
}

fn is_alias(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && text.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
}

fn is_tag(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && text.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_tailnet(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (64..128).contains(&b)
        }
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// Lower-case words with `_`, separated by commas or spaces, deduplicated and sorted as pmd stores them.
pub fn words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = text
        .split([',', ' '])
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect();
    words.sort();
    words.dedup();
    words
}

/// Adds a suggested word to a comma-separated list, unless it is there already.
pub fn add_word(text: &str, word: &str) -> String {
    let mut list = words(text);
    if !list.iter().any(|w| w == word) {
        list.push(word.to_owned());
    }
    list.join(", ")
}

/// The key a new row's fields make: a machine's name, a subscription's id, or `machine/provider/alias`.
pub fn key_of(kind: Kind, values: &BTreeMap<String, String>) -> String {
    let get = |field: &str| values.get(field).map_or("", |v| v.trim());
    match kind {
        Kind::Machine => get("name").to_owned(),
        Kind::Subscription => get("id").to_owned(),
        Kind::Agent => format!("{}/{}/{}", get("machine"), get("provider"), get("alias")),
    }
}

/// The whole row as pmd's `PUT` body, with its key, or the problems that stop the save.
pub fn body(
    kind: Kind,
    values: &BTreeMap<String, String>,
    known: &Known,
) -> Result<(String, Value), Vec<FieldProblem>> {
    let get = |field: &str| values.get(field).map_or("", |v| v.trim());
    let mut problems: Vec<FieldProblem> = Vec::new();
    let mut tags = |field: &'static str| {
        let list = words(get(field));
        if let Some(bad) = list.iter().find(|word| !is_tag(word)) {
            problems.push((
                field,
                format!("{bad:?}: lower-case letters, digits and _, starting with a letter."),
            ));
        } else if list.len() > MAX_TAGS {
            problems.push((field, format!("At most {MAX_TAGS}.")));
        }
        list
    };
    let body = match kind {
        Kind::Machine => {
            let capabilities = tags("capabilities");
            if !is_slug(get("name")) {
                problems.push(("name", "Lower-case letters, digits and -, at most 63.".to_owned()));
            }
            if !OSES.contains(&get("os")) {
                problems.push(("os", "Pick linux, windows or macos.".to_owned()));
            }
            let tailnet = match get("tailnetIp") {
                "" => Value::Null,
                text => match text.parse::<IpAddr>() {
                    Ok(ip) if is_tailnet(ip) => ip.to_string().into(),
                    _ => {
                        problems.push((
                            "tailnetIp",
                            "A Tailscale address (100.64.0.0/10 or fd7a:115c:a1e0::/48), or empty.".to_owned(),
                        ));
                        Value::Null
                    }
                },
            };
            let sessions = match get("maxSessions").parse::<u64>() {
                Ok(n) if n <= MAX_SESSIONS => n,
                _ => {
                    problems.push(("maxSessions", format!("A whole number from 0 to {MAX_SESSIONS}.")));
                    0
                }
            };
            json!({"os": get("os"), "tailnetIp": tailnet, "maxSessions": sessions, "capabilities": capabilities})
        }
        Kind::Subscription => {
            if !is_slug(get("id")) {
                problems.push(("id", "Lower-case letters, digits and -, at most 63.".to_owned()));
            }
            if !is_provider(get("provider")) {
                problems.push((
                    "provider",
                    "Lower-case letters, digits and -, starting with a letter, at most 32.".to_owned(),
                ));
            }
            if get("plan").chars().count() > MAX_PLAN {
                problems.push(("plan", format!("At most {MAX_PLAN} characters.")));
            }
            let metered = get("metered") == "true";
            // usageSource is left out, so pmd keeps the stored one.
            json!({"provider": get("provider"), "plan": get("plan"), "metered": metered})
        }
        Kind::Agent => {
            let roles = tags("roles");
            if !is_slug(get("machine")) {
                problems.push(("machine", "Pick a machine.".to_owned()));
            } else if !known.machines.iter().any(|m| m == get("machine")) {
                problems.push(("machine", format!("No machine {:?} in pmd.", get("machine"))));
            }
            if !is_provider(get("provider")) {
                problems.push((
                    "provider",
                    "The harness, such as claude or codex: lower-case letters, digits and -.".to_owned(),
                ));
            }
            if !is_alias(get("alias")) {
                problems.push(("alias", "Lower-case letters, digits and . _ -, at most 64.".to_owned()));
            }
            if !is_model(get("model")) {
                problems.push(("model", "Letters, digits and . _ : -, at most 64.".to_owned()));
            }
            if !is_slug(get("subscription")) {
                problems.push(("subscription", "Pick a subscription.".to_owned()));
            } else if !known.subscriptions.iter().any(|s| s == get("subscription")) {
                problems.push((
                    "subscription",
                    format!("No subscription {:?} in pmd.", get("subscription")),
                ));
            }
            json!({
                "model": get("model"), "subscription": get("subscription"), "roles": roles,
                "enabled": get("enabled") != "false"
            })
        }
    };
    if problems.is_empty() {
        Ok((key_of(kind, values), body))
    } else {
        Err(problems)
    }
}

/// The field a fleet write error belongs to, beyond a `400` whose message names one: `unknown_machine` and
/// `unknown_subscription` on the agent's pickers, `already_exists` on the key.
pub fn error_field(kind: Kind, error: &WriteError) -> Option<&'static str> {
    match error {
        WriteError::BadRequest { code, .. } if code == "unknown_machine" => Some("machine"),
        WriteError::BadRequest { code, .. } if code == "unknown_subscription" => Some("subscription"),
        WriteError::AlreadyExists => Some(kind.key_field()),
        _ => None,
    }
}
