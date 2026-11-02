//! Transport-independent ref validation, fast-forward rules, and recoverable publication.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceUpdate {
    pub name: String,
    pub before: Option<ObjectId>,
    pub after: ObjectId,
}

impl<B: Backend> Repository<B> {
    pub fn reference_inventory(&self) -> Result<BTreeMap<String, ObjectId>> {
        self.ensure_ready()?;
        self.backend
            .list_meta("refs/")?
            .into_iter()
            .filter(|name| !name.starts_with("refs/remotes/"))
            .map(|name| {
                let id = parse_pointer(
                    &self
                        .backend
                        .read_meta(&name)?
                        .ok_or_else(|| Error::Integrity("missing ref".into()))?,
                )?;
                self.verify_reference(&name, &id)?;
                Ok((name, id))
            })
            .collect()
    }
    pub fn reference_descends_from(
        &self,
        name: &str,
        after: &ObjectId,
        before: &ObjectId,
    ) -> Result<bool> {
        let mut pending = vec![after.clone()];
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if id == *before {
                return Ok(true);
            }
            if seen.insert(id.clone()) {
                if name.starts_with("refs/heads/") || name.starts_with("refs/remotes/") {
                    pending.extend(self.load::<Commit>(&id)?.parents);
                } else if name.starts_with("refs/issues/") {
                    pending.extend(self.load::<IssueEvent>(&id)?.parent);
                } else if name.starts_with("refs/reviews/") {
                    pending.extend(self.load::<ValidationRecord>(&id)?.parent);
                } else {
                    return Ok(false);
                }
            }
        }
        Ok(false)
    }
    /// All objects must already be imported. Publication and any checkout share one journal.
    pub fn publish_references(
        &self,
        updates: &[ReferenceUpdate],
        checkout_branch: Option<&str>,
        allow_incompatible: bool,
    ) -> Result<()> {
        self.ensure_ready()?;
        self.fsck()?;
        let mut all_names: BTreeSet<_> = self.backend.list_meta("refs/")?.into_iter().collect();
        all_names.extend(updates.iter().map(|update| update.name.clone()));
        validate_path_set(all_names.iter().map(String::as_str))
            .map_err(|e| Error::Conflict(format!("conflicting reference names: {e}")))?;
        let mut names = BTreeSet::new();
        for update in updates {
            if !names.insert(&update.name) {
                return Err(Error::Invalid("duplicate ref update".into()));
            }
            self.verify_reference(&update.name, &update.after)?;
            let current = self
                .backend
                .read_meta(&update.name)?
                .map(|s| parse_pointer(&s))
                .transpose()?;
            if current != update.before {
                return Err(Error::Conflict(format!(
                    "{} changed since advertisement; fetch again",
                    update.name
                )));
            }
            if let Some(before) = &update.before
                && !self.reference_descends_from(&update.name, &update.after, before)?
            {
                return Err(Error::Conflict(format!(
                    "{} is not a fast-forward (immutable tags/releases cannot be replaced)",
                    update.name
                )));
            }
        }
        let status = if checkout_branch.is_some() {
            Some(self.status()?)
        } else {
            None
        };
        let mut tx = Transaction::new("publish synchronized references");
        let active = self.head_value_for_sync()?;
        let active_branch = active
            .strip_prefix("ref: ")
            .and_then(|s| s.strip_suffix('\n'));
        for update in updates {
            if active_branch == Some(update.name.as_str())
                && update.before.as_ref() != Some(&update.after)
                && !self.backend.is_bare()
                && checkout_branch.map(|name| format!("refs/heads/{name}"))
                    != Some(update.name.clone())
            {
                return Err(Error::Conflict("refusing to push into a checked-out working branch; use a bare peer or another branch".into()));
            }
        }
        if let Some(branch) = checkout_branch {
            validate_ref_name(branch)?;
            let status = status
                .as_ref()
                .ok_or_else(|| Error::Integrity("missing checkout status".into()))?;
            if !status.staged.is_empty() || !status.unstaged.is_empty() {
                return Err(Error::Conflict(
                    "pull/clone requires clean staging and tracked working files".into(),
                ));
            }
            if self.head_id()?.is_some() && status.branch.as_deref() != Some(branch) {
                return Err(Error::Conflict(
                    "pull must fast-forward the current branch".into(),
                ));
            }
            let name = format!("refs/heads/{branch}");
            let target = updates
                .iter()
                .find(|u| u.name == name)
                .map(|u| u.after.clone())
                .ok_or_else(|| Error::Invalid("checkout branch missing from updates".into()))?;
            let snapshot = self.load::<Commit>(&target)?.snapshot;
            if !allow_incompatible {
                self.require_satisfied(&snapshot)?;
            }
            let before = self.index_tree()?;
            let after = self.tree(&snapshot)?;
            self.check_destinations(&before, &after)?;
            tx.files = self.tree_file_changes(&before, &after)?;
            tx.metadata
                .push(self.meta_change("index", Some(pointer(&snapshot)))?);
            tx.metadata
                .push(self.meta_change("HEAD", Some(format!("ref: {name}\n")))?);
        } else if self.backend.is_bare()
            && let Some(update) = updates
                .iter()
                .find(|u| Some(u.name.as_str()) == active_branch)
        {
            let snapshot = self.load::<Commit>(&update.after)?.snapshot;
            tx.metadata
                .push(self.meta_change("index", Some(pointer(&snapshot)))?);
        }
        for update in updates {
            tx.metadata
                .push(self.meta_change(&update.name, Some(pointer(&update.after)))?);
        }
        if tx.metadata.is_empty() {
            return Ok(());
        }
        self.execute(tx)
    }
    fn head_value_for_sync(&self) -> Result<String> {
        self.backend
            .read_meta("HEAD")?
            .ok_or_else(|| Error::Integrity("missing HEAD".into()))
    }
}
