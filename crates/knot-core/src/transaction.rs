use crate::{
    Backend, Commit, Error, FileState, Repository, Result, Snapshot, canonical, parse_pointer,
    validate_path, validate_ref_name,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    pub path: String,
    pub before: Option<FileState>,
    pub after: Option<FileState>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetaChange {
    pub name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Transaction {
    pub version: u32,
    pub id: Uuid,
    pub purpose: String,
    pub files: Vec<FileChange>,
    pub metadata: Vec<MetaChange>,
}
impl Transaction {
    pub fn new(purpose: &str) -> Self {
        Self {
            version: 1,
            id: Uuid::new_v4(),
            purpose: purpose.into(),
            files: vec![],
            metadata: vec![],
        }
    }
}

impl<B: Backend> Repository<B> {
    pub(crate) fn ensure_ready(&self) -> Result<()> {
        if self.backend.read_meta("transaction.json")?.is_some()
            || self.backend.read_meta("incoming.json")?.is_some()
        {
            return Err(Error::Pending);
        }
        Ok(())
    }

    pub(crate) fn execute(&self, transaction: Transaction) -> Result<()> {
        self.ensure_ready()?;
        self.validate_transaction(&transaction)?;
        for change in &transaction.metadata {
            if self.backend.read_meta(&change.name)? != change.before {
                return Err(Error::Conflict(format!(
                    "{} changed since preview",
                    change.name
                )));
            }
        }
        for change in &transaction.files {
            let hint = change.before.as_ref().is_some_and(|s| s.executable);
            if self.backend.file_state(&change.path, hint)? != change.before {
                return Err(Error::Conflict(format!(
                    "{} changed since preview",
                    change.path
                )));
            }
        }
        let bytes = canonical(&transaction)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| Error::Integrity(e.to_string()))?;
        self.backend.write_meta("transaction.json", Some(text))?;
        self.finish_transaction(transaction, false)
    }

    pub fn recover(&self, abort: bool) -> Result<()> {
        let _reads = self.read_scope();
        let text = self
            .backend
            .read_meta("transaction.json")?
            .ok_or_else(|| Error::NotFound("pending transaction".into()))?;
        let transaction: Transaction = serde_json::from_str(&text)?;
        self.validate_transaction(&transaction)?;
        self.finish_transaction(transaction, abort)
    }

    fn validate_transaction(&self, transaction: &Transaction) -> Result<()> {
        if transaction.version != 1 {
            return Err(Error::Integrity("unsupported transaction version".into()));
        }
        let mut paths = std::collections::BTreeSet::new();
        for change in &transaction.files {
            validate_path(&change.path)?;
            if !paths.insert(&change.path) {
                return Err(Error::Integrity("duplicate transaction path".into()));
            }
            for state in [&change.before, &change.after].into_iter().flatten() {
                self.backend.verify_blob(&state.blob)?;
            }
        }
        let mut names = std::collections::BTreeSet::new();
        for change in &transaction.metadata {
            if ["HEAD", "index"].contains(&change.name.as_str())
                && (change.before.is_none() || change.after.is_none())
            {
                return Err(Error::Integrity(
                    "transaction cannot delete HEAD or staging".into(),
                ));
            }
            if change.name != "HEAD" && change.name != "index" && !change.name.starts_with("refs/")
            {
                return Err(Error::Integrity(
                    "invalid transaction metadata target".into(),
                ));
            }
            for value in [&change.before, &change.after].into_iter().flatten() {
                if change.name == "HEAD" && value.starts_with("ref: refs/heads/") {
                    let tail = value
                        .strip_prefix("ref: refs/heads/")
                        .and_then(|s| s.strip_suffix('\n'))
                        .ok_or_else(|| {
                            Error::Integrity("invalid symbolic HEAD in transaction".into())
                        })?;
                    validate_ref_name(tail)?;
                } else {
                    let id = parse_pointer(value)?;
                    if change.name == "index" {
                        self.load::<Snapshot>(&id)?;
                    } else if change.name == "HEAD" {
                        self.load::<Commit>(&id)?;
                    } else {
                        self.verify_reference(&change.name, &id)?;
                    }
                }
            }
            if !names.insert(&change.name) {
                return Err(Error::Integrity("duplicate transaction metadata".into()));
            }
        }
        Ok(())
    }

    fn finish_transaction(&self, transaction: Transaction, abort: bool) -> Result<()> {
        let log_name = format!("logs/{}.json", transaction.id);
        if let Some(log) = self.backend.read_meta(&log_name)? {
            let record: serde_json::Value = serde_json::from_str(&log)?;
            let expected = if abort { "aborted" } else { "completed" };
            if record["outcome"].as_str() != Some(expected) {
                return Err(Error::Conflict(
                    "transaction already finalized; recover using its recorded outcome".into(),
                ));
            }
        }
        let files: Vec<_> = if abort {
            transaction.files.iter().rev().collect()
        } else {
            transaction.files.iter().collect()
        };
        for change in files {
            let (from, to) = if abort {
                (&change.after, &change.before)
            } else {
                (&change.before, &change.after)
            };
            let hint = from.as_ref().or(to.as_ref()).is_some_and(|s| s.executable);
            let actual = self.backend.file_state(&change.path, hint)?;
            if actual == *to {
                continue;
            }
            if actual != *from {
                return Err(Error::Conflict(format!(
                    "{} has external changes; recovery stopped",
                    change.path
                )));
            }
            self.backend.write_file(&change.path, to.as_ref())?;
        }
        let metadata: Vec<_> = if abort {
            transaction.metadata.iter().rev().collect()
        } else {
            transaction.metadata.iter().collect()
        };
        for change in metadata {
            let (from, to) = if abort {
                (&change.after, &change.before)
            } else {
                (&change.before, &change.after)
            };
            let actual = self.backend.read_meta(&change.name)?;
            if actual == *to {
                continue;
            }
            if actual != *from {
                return Err(Error::Conflict(format!(
                    "{} changed during recovery",
                    change.name
                )));
            }
            self.backend.write_meta(&change.name, to.as_deref())?;
        }
        let record = serde_json::json!({ "transaction": transaction, "outcome": if abort { "aborted" } else { "completed" } });
        let log =
            String::from_utf8(canonical(&record)?).map_err(|e| Error::Integrity(e.to_string()))?;
        self.backend
            .write_meta(&format!("logs/{}.json", transaction.id), Some(&log))?;
        self.backend.write_meta("transaction.json", None)
    }
}
