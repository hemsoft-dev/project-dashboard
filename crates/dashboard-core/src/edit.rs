//! One management dialog's edit of one pmd row (#33), without a window: what the user typed, whether it is unsaved,
//! the save in flight, errors placed on fields or on the dialog, and the shared "Changed elsewhere" flow.
//!
//! Each dialog supplies only its fields and how their text becomes a JSON body. On `409 stale_version` the form
//! keeps the user's values, the caller re-reads the row with the read token, and [`Form::conflict`] shows both side by
//! side until the user reapplies their edits to the new version or discards them.
use std::collections::BTreeMap;

use serde_json::Value;

use crate::write::{Precondition, WriteError};

/// A row as pmd last returned it: its version, for `If-Match`, and the text of the fields the dialog edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub version: u64,
    pub values: BTreeMap<String, String>,
}

impl Row {
    /// The dialog's `fields` from a pmd row. `null` reads as empty text.
    pub fn from_json(value: &Value, fields: &[&str]) -> Result<Self, WriteError> {
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or(WriteError::Malformed)?;
        let values = fields
            .iter()
            .map(|field| {
                let text = match value.get(*field) {
                    None | Some(Value::Null) => String::new(),
                    Some(Value::String(text)) => text.clone(),
                    Some(other) => other.to_string(),
                };
                ((*field).to_owned(), text)
            })
            .collect();
        Ok(Self { version, values })
    }
}

/// The user's values next to the row as it is now, after someone else changed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub theirs: Row,
    pub mine: BTreeMap<String, String>,
}

impl Conflict {
    /// The fields where the user's value and the new value differ: `(field, mine, theirs)`, in field order.
    pub fn differences(&self) -> Vec<(&str, &str, &str)> {
        self.mine
            .iter()
            .filter_map(|(field, mine)| {
                let theirs = self.theirs.values.get(field).map_or("", String::as_str);
                (mine != theirs).then_some((field.as_str(), mine.as_str(), theirs))
            })
            .collect()
    }
}

/// What the caller does after a failed save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Show the error; nothing else to do.
    Show,
    /// `409 stale_version`: re-read the row with the read token, then call [`Form::conflict`].
    Reread,
}

/// One dialog's edit of one row, or of a new row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    fields: Vec<String>,
    /// The row as last read; `None` while creating.
    base: Option<Row>,
    /// What a new row starts with, to tell an untouched create from an edited one.
    defaults: BTreeMap<String, String>,
    values: BTreeMap<String, String>,
    saving: bool,
    message: Option<String>,
    field_errors: BTreeMap<String, String>,
    conflict: Option<Conflict>,
}

impl Form {
    /// Edits an existing row.
    pub fn edit(row: Row, fields: &[&str]) -> Self {
        let values = Self::only(&row.values, fields);
        Self {
            fields: fields.iter().map(|f| (*f).to_owned()).collect(),
            defaults: values.clone(),
            values,
            base: Some(row),
            saving: false,
            message: None,
            field_errors: BTreeMap::new(),
            conflict: None,
        }
    }

    /// Creates a row, starting from `defaults` (missing fields start empty).
    pub fn create(fields: &[&str], defaults: &BTreeMap<String, String>) -> Self {
        let values = Self::only(defaults, fields);
        Self {
            fields: fields.iter().map(|f| (*f).to_owned()).collect(),
            defaults: values.clone(),
            values,
            base: None,
            saving: false,
            message: None,
            field_errors: BTreeMap::new(),
            conflict: None,
        }
    }

    fn only(values: &BTreeMap<String, String>, fields: &[&str]) -> BTreeMap<String, String> {
        fields
            .iter()
            .map(|f| ((*f).to_owned(), values.get(*f).cloned().unwrap_or_default()))
            .collect()
    }

    pub fn fields(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(String::as_str)
    }

    pub fn value(&self, field: &str) -> &str {
        self.values.get(field).map_or("", String::as_str)
    }

    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }

    /// The row as last read; `None` while creating.
    pub fn base(&self) -> Option<&Row> {
        self.base.as_ref()
    }

    /// The row's version as last read; `None` while creating.
    pub fn version(&self) -> Option<u64> {
        self.base.as_ref().map(|row| row.version)
    }

    /// Changes a field's text. Its own error goes, since it may no longer apply. Ignored while saving or while a
    /// conflict waits for the user's choice, so neither can be changed underneath.
    pub fn set(&mut self, field: &str, text: impl Into<String>) {
        if self.saving || self.conflict.is_some() || !self.values.contains_key(field) {
            return;
        }
        self.values.insert(field.to_owned(), text.into());
        self.field_errors.remove(field);
    }

    /// True when the values differ from the row as read (or a create's starting values), so closing would lose them.
    pub fn is_dirty(&self) -> bool {
        let base = self.base.as_ref().map_or(&self.defaults, |row| &row.values);
        self.fields
            .iter()
            .any(|field| self.value(field) != base.get(field).map_or("", String::as_str))
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub fn conflict_state(&self) -> Option<&Conflict> {
        self.conflict.as_ref()
    }

    /// The dialog-level message: an error that names no field, or "Changed elsewhere".
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn field_error(&self, field: &str) -> Option<&str> {
        self.field_errors.get(field).map(String::as_str)
    }

    /// A problem the dialog found in a field before sending, such as text where a number goes.
    pub fn reject(&mut self, field: &str, reason: impl Into<String>) {
        self.field_errors.insert(field.to_owned(), reason.into());
    }

    /// Starts a save and returns its precondition, or `None` when one is already in flight (a double click), a
    /// conflict waits for the user's choice, or a field has a known problem.
    pub fn begin_save(&mut self) -> Option<Precondition> {
        if self.saving || self.conflict.is_some() || !self.field_errors.is_empty() {
            return None;
        }
        self.saving = true;
        self.message = None;
        Some(match &self.base {
            Some(row) => Precondition::IfMatch(row.version),
            None => Precondition::IfNoneMatch,
        })
    }

    /// The save landed: pmd's answer is the new row, and nothing is unsaved.
    pub fn saved(&mut self, row: Row) {
        self.values = Self::only(&row.values, &self.fields.iter().map(String::as_str).collect::<Vec<_>>());
        self.defaults = self.values.clone();
        self.base = Some(row);
        self.saving = false;
        self.message = None;
        self.field_errors.clear();
    }

    /// The save failed. A `400` that names a field goes next to it; anything else goes on the dialog. The user's
    /// values stay as they are.
    pub fn failed(&mut self, error: &WriteError) -> Next {
        let fields: Vec<&str> = self.fields.iter().map(String::as_str).collect();
        if *error == WriteError::StaleVersion {
            // Still busy until the re-read lands, so another save can't race it.
            self.message = Some("Changed elsewhere. Loading the new values…".to_owned());
            return Next::Reread;
        }
        self.saving = false;
        match error.field(&fields) {
            Some(field) => {
                self.field_errors.insert(field.to_owned(), error.to_string());
            }
            None => self.message = Some(error.to_string()),
        }
        Next::Show
    }

    /// The re-read after `409 stale_version` landed: show the user's values next to the new ones.
    pub fn conflict(&mut self, theirs: Row) {
        self.saving = false;
        self.message = Some("Changed elsewhere. Reapply your edits to the new version, or discard them.".to_owned());
        self.conflict = Some(Conflict {
            theirs,
            mine: self.values.clone(),
        });
    }

    /// The re-read after `409 stale_version` failed too.
    pub fn reread_failed(&mut self, error: &WriteError) {
        self.saving = false;
        self.message = Some(format!("Changed elsewhere, and reloading it failed: {error}"));
    }

    /// Keeps the user's values on top of the new version; the next save sends them with its `If-Match`.
    pub fn reapply(&mut self) {
        if let Some(conflict) = self.conflict.take() {
            self.base = Some(conflict.theirs);
            self.message = None;
        }
    }

    /// Drops the user's values for the new ones.
    pub fn discard(&mut self) {
        if let Some(conflict) = self.conflict.take() {
            self.saved(conflict.theirs);
        }
    }
}
