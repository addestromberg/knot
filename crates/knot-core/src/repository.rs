use crate::*;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub(crate) type Tree = BTreeMap<String, ObjectId>;

pub struct Repository<B: Backend> {
    pub(crate) backend: B,
}

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub path: String,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub head: Option<ObjectId>,
    pub branch: Option<String>,
    pub snapshot: Option<ObjectId>,
    pub staged_snapshot: ObjectId,
    pub staged: Vec<Change>,
    pub unstaged: Vec<Change>,
    pub untracked: Vec<String>,
    pub recovery_required: bool,
    pub dependency_conflicts: Vec<DependencyEdge>,
    pub requirements_satisfied: bool,
    pub dependency_graph: GraphReport,
    pub verification: VerificationReport,
}
#[derive(Clone, Debug, Serialize)]
pub struct HistoryEntry {
    pub id: ObjectId,
    pub commit: Commit,
}
#[derive(Clone, Debug, Serialize)]
pub struct DiffEntry {
    pub path: String,
    pub before: Option<FileState>,
    pub after: Option<FileState>,
    pub before_size: Option<u64>,
    pub after_size: Option<u64>,
    pub patch: Option<String>,
    pub metadata_changes: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct CheckoutPlan {
    target: String,
    allow_incompatible: bool,
    transaction: Transaction,
}
#[derive(Clone, Debug, Serialize)]
pub struct RestoreResult {
    pub staged: bool,
    pub staged_snapshot: Option<ObjectId>,
    pub changes: Vec<FileChange>,
}
#[derive(Clone, Debug, Serialize)]
pub struct FsckReport {
    pub objects: usize,
    pub refs: usize,
    pub pending_transaction: bool,
}

pub(crate) fn timestamp() -> Result<String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Invalid(e.to_string()))?
        .as_secs()
        .to_string())
}
pub(crate) fn pointer(id: &ObjectId) -> String {
    format!("{id}\n")
}
pub(crate) fn parse_pointer(text: &str) -> Result<ObjectId> {
    let id: ObjectId = text
        .strip_suffix('\n')
        .ok_or_else(|| Error::Integrity("object pointer must end with exactly one newline".into()))?
        .parse()
        .map_err(|_| Error::Integrity("invalid object pointer".into()))?;
    if pointer(&id) != text {
        return Err(Error::Integrity("noncanonical object pointer".into()));
    }
    Ok(id)
}
fn in_scope(path: &str, scope: &str) -> bool {
    scope == "."
        || path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn changes(before: &Tree, after: &Tree) -> Vec<Change> {
    before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|path| {
            if before.get(path) == after.get(path) {
                return None;
            }
            Some(Change {
                path: path.clone(),
                kind: match (before.get(path), after.get(path)) {
                    (None, _) => "added",
                    (_, None) => "deleted",
                    _ => "modified",
                }
                .into(),
            })
        })
        .collect()
}

impl<B: Backend> Repository<B> {
    pub fn open(backend: B) -> Result<Self> {
        let repo = Self { backend };
        repo.head_value()?;
        repo.index_id()?;
        Ok(repo)
    }
    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn load<T: StoredObject>(&self, id: &ObjectId) -> Result<T> {
        let version = self.backend.object_version(id)?;
        if !T::SUPPORTED_VERSIONS.contains(&version) {
            return Err(Error::Integrity(format!(
                "unsupported {} schema version: {version}",
                T::KIND.as_str()
            )));
        }
        let (kind, bytes) = self.backend.get_object(id)?;
        if kind != T::KIND {
            return Err(Error::Integrity(format!(
                "{id}: expected {}, got {}",
                T::KIND.as_str(),
                kind.as_str()
            )));
        }
        let value: T = serde_json::from_slice(&bytes)?;
        if value.schema_version() != version || canonical(&value)? != bytes {
            return Err(Error::Integrity(format!("{id}: noncanonical object")));
        }
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn put<T: StoredObject>(&self, value: &T) -> Result<ObjectId> {
        value.validate()?;
        let bytes = canonical(value)?;
        if bytes.len() > MAX_STRUCTURED_OBJECT_SIZE {
            return Err(Error::Invalid(
                "structured object exceeds 16 MiB limit".into(),
            ));
        }
        self.backend
            .put_versioned_object(T::KIND, value.schema_version(), &bytes)
    }
    fn head_value(&self) -> Result<String> {
        self.backend
            .read_meta("HEAD")?
            .ok_or_else(|| Error::Integrity("missing HEAD".into()))
    }
    fn head_ref(&self) -> Result<Option<String>> {
        let value = self.head_value()?;
        if let Some(name) = value
            .strip_suffix('\n')
            .and_then(|s| s.strip_prefix("ref: refs/heads/"))
        {
            validate_ref_name(name).map_err(|e| Error::Integrity(e.to_string()))?;
            Ok(Some(format!("refs/heads/{name}")))
        } else {
            parse_pointer(&value)?;
            Ok(None)
        }
    }
    pub fn head_id(&self) -> Result<Option<ObjectId>> {
        if let Some(name) = self.head_ref()? {
            self.backend
                .read_meta(&name)?
                .map(|s| parse_pointer(&s))
                .transpose()
        } else {
            Ok(Some(parse_pointer(&self.head_value()?)?))
        }
    }
    pub(crate) fn index_id(&self) -> Result<ObjectId> {
        parse_pointer(
            &self
                .backend
                .read_meta("index")?
                .ok_or_else(|| Error::Integrity("missing staging snapshot pointer".into()))?,
        )
    }
    pub(crate) fn head_tree(&self) -> Result<Tree> {
        match self.head_id()? {
            Some(id) => self.tree(&self.load::<Commit>(&id)?.snapshot),
            None => Ok(Tree::new()),
        }
    }
    pub(crate) fn index_tree(&self) -> Result<Tree> {
        self.tree(&self.index_id()?)
    }
    pub(crate) fn tree(&self, id: &ObjectId) -> Result<Tree> {
        let snapshot = self.load::<Snapshot>(id)?;
        self.validate_snapshot(&snapshot)?;
        Ok(snapshot
            .entries
            .into_iter()
            .map(|e| (e.path, e.revision))
            .collect())
    }
    fn validate_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        snapshot.validate()?;
        let mut assets = BTreeSet::new();
        let mut test_names = BTreeSet::new();
        for entry in &snapshot.entries {
            let revision = self.load::<Revision>(&entry.revision)?;
            self.verify_revision_metadata(&revision)?;
            if let Some(name) = self.validate_versioned_manifest(&entry.path, &revision)?
                && !test_names.insert(name)
            {
                return Err(Error::Integrity("duplicate test name in snapshot".into()));
            }
            if !assets.insert(revision.asset) {
                return Err(Error::Integrity("asset occurs twice in snapshot".into()));
            }
        }
        Ok(())
    }
    pub(crate) fn save_tree(&self, tree: &Tree) -> Result<ObjectId> {
        validate_path_set(tree.keys().map(String::as_str))?;
        let snapshot = Snapshot {
            entries: tree
                .iter()
                .map(|(path, revision)| SnapshotEntry {
                    path: path.clone(),
                    revision: revision.clone(),
                })
                .collect(),
        };
        self.validate_snapshot(&snapshot)?;
        self.put(&snapshot)
    }
    fn state(&self, revision: &ObjectId) -> Result<FileState> {
        let revision = self.load::<Revision>(revision)?;
        Ok(FileState {
            blob: revision.blob,
            executable: revision.executable,
        })
    }
    pub(crate) fn meta_change(&self, name: &str, after: Option<String>) -> Result<MetaChange> {
        Ok(MetaChange {
            name: name.into(),
            before: self.backend.read_meta(name)?,
            after,
        })
    }
    pub(crate) fn stage_tree(&self, tree: &Tree, purpose: &str) -> Result<ObjectId> {
        let id = self.save_tree(tree)?;
        let mut tx = Transaction::new(purpose);
        tx.metadata
            .push(self.meta_change("index", Some(pointer(&id)))?);
        self.execute(tx)?;
        Ok(id)
    }

    pub fn status(&self) -> Result<Status> {
        let head = self.head_id()?;
        let index = self.index_tree()?;
        let mut unstaged = Vec::new();
        for (path, rev) in &index {
            if self.backend.is_bare() {
                break;
            }
            let expected = self.state(rev)?;
            let actual = self.backend.file_state(path, expected.executable)?;
            if actual.as_ref() != Some(&expected) {
                unstaged.push(Change {
                    path: path.clone(),
                    kind: if actual.is_some() {
                        "modified"
                    } else {
                        "deleted"
                    }
                    .into(),
                });
            }
        }
        let files = if self.backend.is_bare() {
            vec![]
        } else {
            self.backend.list_files(false)?
        };
        let untracked = files
            .into_iter()
            .filter(|path| !index.contains_key(path))
            .collect();
        let dependency_graph = self.graph_snapshot(&self.index_id()?)?;
        let dependency_conflicts: Vec<_> = dependency_graph
            .edges
            .iter()
            .filter(|e| e.status != RequirementStatus::Satisfied)
            .cloned()
            .collect();
        let requirements_satisfied = dependency_conflicts.is_empty();
        let verification = self.verification_snapshot(&dependency_graph)?;
        Ok(Status {
            dependency_conflicts,
            requirements_satisfied,
            dependency_graph,
            verification,
            branch: self.head_ref()?.map(|r| r["refs/heads/".len()..].into()),
            snapshot: head
                .as_ref()
                .map(|id| self.load::<Commit>(id).map(|c| c.snapshot))
                .transpose()?,
            head,
            staged_snapshot: self.index_id()?,
            staged: changes(&self.head_tree()?, &index),
            unstaged,
            untracked,
            recovery_required: self.backend.read_meta("transaction.json")?.is_some(),
        })
    }

    pub fn add(&self, scopes: &[String]) -> Result<ObjectId> {
        if self.backend.is_bare() {
            return Err(Error::Invalid("bare repository has no working tree".into()));
        }
        self.ensure_ready()?;
        if scopes.is_empty() {
            return Err(Error::Invalid("provide at least one path".into()));
        }
        for scope in scopes {
            if scope != "." {
                validate_path(scope)?;
            }
        }
        let mut index = self.index_tree()?;
        let head = self.head_tree()?;
        let mut baselines = BTreeMap::new();
        for id in head.values() {
            let revision = self.load::<Revision>(id)?;
            baselines.insert(revision.asset.clone(), (id.clone(), revision));
        }
        let files = self.backend.list_files(false)?;
        let mut candidates: BTreeSet<_> = files.into_iter().collect();
        candidates.extend(index.keys().cloned());
        for scope in scopes {
            if !candidates.iter().any(|p| in_scope(p, scope)) && scope != "." {
                return Err(Error::NotFound(format!(
                    "tracked or nonignored path {scope}"
                )));
            }
        }
        for path in candidates
            .into_iter()
            .filter(|p| scopes.iter().any(|s| in_scope(p, s)))
        {
            let old_id = index.get(&path).or_else(|| head.get(&path));
            let old = old_id.map(|id| self.load::<Revision>(id)).transpose()?;
            let hint = old.as_ref().is_some_and(|r| r.executable);
            let Some(state) = self.backend.store_file(&path, hint)? else {
                index.remove(&path);
                continue;
            };
            let asset = if let Some(ref revision) = old {
                revision.asset.clone()
            } else {
                self.put(&Asset {
                    identity: Uuid::new_v4(),
                })?
            };
            let baseline = baselines.get(&asset).cloned();
            let mut revision = Revision {
                asset,
                blob: state.blob,
                parents: vec![],
                executable: state.executable,
                dependencies: old
                    .as_ref()
                    .map(|r| r.dependencies.clone())
                    .unwrap_or_default(),
                interfaces: old
                    .as_ref()
                    .map(|r| r.interfaces.clone())
                    .unwrap_or_default(),
            };
            let revision_id = if let Some((id, baseline)) = baseline {
                if same_revision_state(&revision, &baseline) {
                    id
                } else {
                    revision.parents = vec![id];
                    self.put(&revision)?
                }
            } else {
                self.put(&revision)?
            };
            index.insert(path, revision_id);
        }
        self.stage_tree(&index, "add")
    }

    pub fn commit(&self, message: &str) -> Result<ObjectId> {
        self.commit_with_options(message, false)
    }
    pub fn commit_with_options(&self, message: &str, allow_incompatible: bool) -> Result<ObjectId> {
        self.ensure_ready()?;
        if message.trim().is_empty() {
            return Err(Error::Invalid("commit message must not be empty".into()));
        }
        let index = self.index_id()?;
        let tree = self.tree(&index)?;
        if tree == self.head_tree()? {
            return Err(Error::Conflict("no staged changes".into()));
        }
        for revision_id in tree.values() {
            let revision = self.load::<Revision>(revision_id)?;
            self.load::<Asset>(&revision.asset)?;
            self.backend.verify_blob(&revision.blob)?;
            for parent in revision.parents {
                if self.load::<Revision>(&parent)?.asset != revision.asset {
                    return Err(Error::Integrity(
                        "revision parent belongs to another asset".into(),
                    ));
                }
            }
        }
        if !allow_incompatible {
            self.require_satisfied(&index)?;
        }
        let parent = self.head_id()?;
        let mut overrides = Vec::new();
        for entry in self
            .verification_snapshot(&self.graph_snapshot(&index)?)?
            .dependencies
        {
            if entry.overridden && entry.state == VerificationState::Verified {
                overrides.extend(entry.record);
            }
        }
        overrides.sort();
        let commit = Commit {
            snapshot: index,
            parents: parent.into_iter().collect(),
            message: message.into(),
            author: self.backend.author()?,
            timestamp: timestamp()?,
            overrides,
        };
        let id = self.put(&commit)?;
        let target = self.head_ref()?.unwrap_or_else(|| "HEAD".into());
        let mut tx = Transaction::new("commit");
        tx.metadata
            .push(self.meta_change(&target, Some(pointer(&id)))?);
        self.execute(tx)?;
        Ok(id)
    }

    pub fn resolve(&self, target: &str) -> Result<ObjectId> {
        if let Some(name) = target.strip_prefix("release/") {
            validate_ref_name(name)?;
            let text = self
                .backend
                .read_meta(&format!("refs/releases/{name}"))?
                .ok_or_else(|| Error::NotFound(format!("release {name}")))?;
            return Ok(self.load::<Release>(&parse_pointer(&text)?)?.commit);
        }
        if target == "HEAD" {
            return self
                .head_id()?
                .ok_or_else(|| Error::NotFound("HEAD has no commit".into()));
        }
        if let Ok(id) = target.parse::<ObjectId>() {
            self.load::<Commit>(&id)?;
            return Ok(id);
        }
        validate_ref_name(target)?;
        let mut found = Vec::new();
        for prefix in ["refs/heads", "refs/tags"] {
            if let Some(text) = self.backend.read_meta(&format!("{prefix}/{target}"))? {
                let id = parse_pointer(&text)?;
                self.load::<Commit>(&id)?;
                found.push(id);
            }
        }
        match found.len() {
            0 => Err(Error::NotFound(format!("commit or ref {target}"))),
            1 => Ok(found.remove(0)),
            _ => Err(Error::Conflict(format!(
                "ambiguous branch/tag name: {target}"
            ))),
        }
    }

    pub fn refs(&self, tags: bool) -> Result<BTreeMap<String, ObjectId>> {
        let prefix = if tags { "refs/tags/" } else { "refs/heads/" };
        self.backend
            .list_meta(prefix)?
            .into_iter()
            .map(|name| {
                let value = self
                    .backend
                    .read_meta(&name)?
                    .ok_or_else(|| Error::Integrity(format!("missing {name}")))?;
                Ok((name[prefix.len()..].into(), parse_pointer(&value)?))
            })
            .collect()
    }
    pub fn create_ref(&self, name: &str, target: &str, tag: bool) -> Result<ObjectId> {
        self.ensure_ready()?;
        validate_ref_name(name)?;
        let id = self.resolve(target)?;
        // Avoid names that are ambiguous on checkout or on case-insensitive filesystems.
        for existing in self.refs(false)?.keys().chain(self.refs(true)?.keys()) {
            let a = portable_key(existing);
            let b = portable_key(name);
            if a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/")) {
                return Err(Error::Conflict(format!(
                    "ref already exists or collides: {existing}"
                )));
            }
        }
        let key = format!("refs/{}/{name}", if tag { "tags" } else { "heads" });
        let mut tx = Transaction::new(if tag { "tag" } else { "branch" });
        tx.metadata
            .push(self.meta_change(&key, Some(pointer(&id)))?);
        self.execute(tx)?;
        Ok(id)
    }

    pub fn history(
        &self,
        target: &str,
        path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>> {
        let mut next = if target == "HEAD" {
            self.head_id()?
        } else {
            Some(self.resolve(target)?)
        };
        let asset = if let Some(path) = path {
            validate_path(path)?;
            let tree = if let Some(ref id) = next {
                self.tree(&self.load::<Commit>(id)?.snapshot)?
            } else {
                Tree::new()
            };
            let revision = tree
                .get(path)
                .ok_or_else(|| Error::NotFound(format!("asset at {path}")))?;
            Some(self.load::<Revision>(revision)?.asset)
        } else {
            None
        };
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        while let Some(id) = next {
            if !seen.insert(id.clone()) {
                return Err(Error::Integrity("commit ancestry cycle".into()));
            }
            let commit = self.load::<Commit>(&id)?;
            next = commit.parents.first().cloned();
            let relevant = if let Some(ref asset) = asset {
                let revision_for = |tree: Tree| -> Result<Option<(String, ObjectId)>> {
                    for (path, id) in tree {
                        if self.load::<Revision>(&id)?.asset == *asset {
                            return Ok(Some((path, id)));
                        }
                    }
                    Ok(None)
                };
                let current = revision_for(self.tree(&commit.snapshot)?)?;
                let previous = if let Some(ref parent) = next {
                    revision_for(self.tree(&self.load::<Commit>(parent)?.snapshot)?)?
                } else {
                    None
                };
                current != previous
            } else {
                true
            };
            if relevant {
                out.push(HistoryEntry { id, commit });
            }
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    pub fn diff(&self, staged: bool) -> Result<Vec<DiffEntry>> {
        let index = self.index_tree()?;
        let before = if staged {
            self.head_tree()?
        } else {
            index.clone()
        };
        let paths: BTreeSet<_> = before.keys().chain(index.keys()).cloned().collect();
        let mut result = Vec::new();
        for path in paths {
            let old = before.get(&path).map(|id| self.state(id)).transpose()?;
            let new = if staged {
                index.get(&path).map(|id| self.state(id)).transpose()?
            } else {
                self.backend
                    .store_file(&path, old.as_ref().is_some_and(|s| s.executable))?
            };
            let mut metadata_changes = Vec::new();
            if staged
                && before.get(&path) != index.get(&path)
                && let (Some(a), Some(b)) = (before.get(&path), index.get(&path))
            {
                let a = self.load::<Revision>(a)?;
                let b = self.load::<Revision>(b)?;
                if a.asset != b.asset {
                    metadata_changes.push("identity".into());
                }
                if a.dependencies != b.dependencies {
                    metadata_changes.push("constraints".into());
                }
                if a.interfaces != b.interfaces {
                    metadata_changes.push("interfaces".into());
                }
                if old == new && metadata_changes.is_empty() {
                    metadata_changes.push("revision".into());
                }
            }
            if old == new && metadata_changes.is_empty() {
                continue;
            }
            let size = |state: &Option<FileState>| -> Result<Option<u64>> {
                state
                    .as_ref()
                    .map(|s| self.backend.verify_blob(&s.blob))
                    .transpose()
            };
            let before_size = size(&old)?;
            let after_size = size(&new)?;
            let patch = if before_size.unwrap_or(0) <= 1024 * 1024
                && after_size.unwrap_or(0) <= 1024 * 1024
            {
                let bytes = |state: &Option<FileState>| -> Result<Vec<u8>> {
                    match state {
                        Some(s) => Ok(self.backend.get_object(&s.blob)?.1),
                        None => Ok(vec![]),
                    }
                };
                let left = bytes(&old)?;
                let right = bytes(&new)?;
                match (std::str::from_utf8(&left), std::str::from_utf8(&right)) {
                    (Ok(a), Ok(b)) if !a.contains('\0') && !b.contains('\0') => Some(
                        similar::TextDiff::from_lines(a, b)
                            .unified_diff()
                            .header(&format!("a/{path}"), &format!("b/{path}"))
                            .to_string(),
                    ),
                    _ => None,
                }
            } else {
                None
            };
            result.push(DiffEntry {
                path,
                before_size,
                after_size,
                before: old,
                after: new,
                patch,
                metadata_changes,
            });
        }
        Ok(result)
    }

    pub fn plan_checkout(&self, target: &str) -> Result<CheckoutPlan> {
        self.plan_checkout_with_options(target, false)
    }
    pub fn plan_checkout_with_options(
        &self,
        target: &str,
        allow_incompatible: bool,
    ) -> Result<CheckoutPlan> {
        if self.backend.is_bare() {
            return Err(Error::Invalid("bare repository has no working tree".into()));
        }
        self.ensure_ready()?;
        let status = self.status()?;
        if !status.staged.is_empty() || !status.unstaged.is_empty() {
            return Err(Error::Conflict(
                "checkout requires a clean staging area and tracked working tree".into(),
            ));
        }
        let commit = self.resolve(target)?;
        let snapshot = self.load::<Commit>(&commit)?.snapshot;
        if !allow_incompatible {
            self.require_satisfied(&snapshot)?;
        }
        let before = self.index_tree()?;
        let after = self.tree(&snapshot)?;
        self.check_destinations(&before, &after)?;
        let mut tx = Transaction::new("checkout");
        tx.files = self.tree_file_changes(&before, &after)?;
        let branch = format!("refs/heads/{target}");
        let is_branch = target != "HEAD"
            && target.parse::<ObjectId>().is_err()
            && self.backend.read_meta(&branch)?.is_some();
        let head = if is_branch {
            format!("ref: {branch}\n")
        } else if target == "HEAD" {
            self.head_value()?
        } else {
            pointer(&commit)
        };
        tx.metadata
            .push(self.meta_change("index", Some(pointer(&snapshot)))?);
        tx.metadata.push(self.meta_change("HEAD", Some(head))?);
        if is_branch {
            tx.metadata
                .push(self.meta_change(&branch, Some(pointer(&commit)))?);
        }
        if let Some(source_ref) = self.head_ref()?
            && !tx.metadata.iter().any(|m| m.name == source_ref)
        {
            let value = self.backend.read_meta(&source_ref)?;
            tx.metadata.push(self.meta_change(&source_ref, value)?);
        }
        Ok(CheckoutPlan {
            target: target.into(),
            allow_incompatible,
            transaction: tx,
        })
    }
    pub fn apply_checkout(&self, plan: CheckoutPlan) -> Result<()> {
        let fresh = self.plan_checkout_with_options(&plan.target, plan.allow_incompatible)?;
        if fresh.transaction.files != plan.transaction.files
            || fresh.transaction.metadata != plan.transaction.metadata
        {
            return Err(Error::Conflict(
                "checkout plan is stale; preview again".into(),
            ));
        }
        self.execute(plan.transaction)
    }
    pub(crate) fn check_destinations(&self, before: &Tree, after: &Tree) -> Result<()> {
        let mut spellings = BTreeMap::new();
        for path in before.keys().chain(after.keys()) {
            for prefix in path
                .match_indices('/')
                .map(|(i, _)| &path[..i])
                .chain(std::iter::once(path.as_str()))
            {
                if let Some(existing) = spellings.insert(portable_key(prefix), prefix.to_owned())
                    && existing != prefix
                {
                    return Err(Error::Conflict("case-only path transitions require a distinct intermediate name and snapshot".into()));
                }
            }
        }
        let actual = self.backend.list_files(true)?;
        // Include ignored files: ignore rules never permit overwriting local data.
        let untracked: Vec<_> = actual.iter().filter(|p| !before.contains_key(*p)).collect();
        validate_path_set(
            after
                .keys()
                .map(String::as_str)
                .chain(untracked.iter().map(|p| p.as_str())),
        )
        .map_err(|e| Error::Conflict(format!("untracked path blocks checkout: {e}")))?;
        Ok(())
    }
    pub(crate) fn tree_file_changes(&self, before: &Tree, after: &Tree) -> Result<Vec<FileChange>> {
        let paths: BTreeSet<_> = before.keys().chain(after.keys()).cloned().collect();
        let mut changes = Vec::new();
        for path in paths {
            changes.push(FileChange {
                before: before.get(&path).map(|id| self.state(id)).transpose()?,
                after: after.get(&path).map(|id| self.state(id)).transpose()?,
                path,
            });
        }
        // Remove files before creating a directory in their place.
        changes.sort_by(|a, b| {
            a.after
                .is_some()
                .cmp(&b.after.is_some())
                .then_with(|| a.path.cmp(&b.path))
        });
        Ok(changes)
    }

    pub fn restore(
        &self,
        scopes: &[String],
        source: Option<&str>,
        staged: bool,
        preview: bool,
    ) -> Result<RestoreResult> {
        if self.backend.is_bare() {
            return Err(Error::Invalid("bare repository has no working tree".into()));
        }
        self.ensure_ready()?;
        if scopes.is_empty() {
            return Err(Error::Invalid("provide paths to restore".into()));
        }
        for scope in scopes {
            if scope != "." {
                validate_path(scope)?;
            }
        }
        let index = self.index_tree()?;
        let source_tree = if let Some(source) = source {
            self.tree(&self.load::<Commit>(&self.resolve(source)?)?.snapshot)?
        } else if staged {
            self.head_tree()?
        } else {
            index.clone()
        };
        let paths: BTreeSet<_> = index
            .keys()
            .chain(source_tree.keys())
            .filter(|p| scopes.iter().any(|s| in_scope(p, s)))
            .cloned()
            .collect();
        if paths.is_empty() {
            return Err(Error::NotFound("no matching tracked paths".into()));
        }
        let mut tx = Transaction::new("restore");
        let mut staged_snapshot = None;
        let mut staged_changes = Vec::new();
        if staged {
            let mut new_index = index.clone();
            for path in &paths {
                if let Some(id) = source_tree.get(path) {
                    new_index.insert(path.clone(), id.clone());
                } else {
                    new_index.remove(path);
                }
            }
            let id = self.save_tree(&new_index)?;
            staged_changes = self
                .tree_file_changes(&index, &new_index)?
                .into_iter()
                .filter(|c| c.before != c.after)
                .collect();
            staged_snapshot = Some(id.clone());
            tx.metadata
                .push(self.meta_change("index", Some(pointer(&id)))?);
        } else {
            let target: Tree = paths
                .iter()
                .filter_map(|p| source_tree.get(p).map(|r| (p.clone(), r.clone())))
                .collect();
            self.check_destinations(&index, &target)?;
            for path in paths {
                let after = source_tree.get(&path).map(|r| self.state(r)).transpose()?;
                let hint = index
                    .get(&path)
                    .map(|r| self.state(r))
                    .transpose()?
                    .or_else(|| after.clone())
                    .is_some_and(|s| s.executable);
                let before = self.backend.store_file(&path, hint)?;
                tx.files.push(FileChange {
                    path,
                    before,
                    after,
                });
            }
            tx.files.sort_by(|a, b| {
                a.after
                    .is_some()
                    .cmp(&b.after.is_some())
                    .then_with(|| a.path.cmp(&b.path))
            });
        }
        let result = RestoreResult {
            staged,
            staged_snapshot,
            changes: if staged {
                staged_changes
            } else {
                tx.files.clone()
            },
        };
        if !preview {
            self.execute(tx)?;
        }
        Ok(result)
    }

    pub fn move_asset(&self, from: &str, to: &str, preview: bool) -> Result<Vec<FileChange>> {
        if self.backend.is_bare() {
            return Err(Error::Invalid("bare repository has no working tree".into()));
        }
        self.ensure_ready()?;
        validate_path(from)?;
        validate_path(to)?;
        let mut tree = self.index_tree()?;
        let revision = tree
            .get(from)
            .cloned()
            .ok_or_else(|| Error::NotFound(from.into()))?;
        let expected = self.state(&revision)?;
        if self.backend.file_state(from, expected.executable)?.as_ref() != Some(&expected) {
            return Err(Error::Conflict(format!("{from} has unstaged changes")));
        }
        if tree.contains_key(to) {
            return Err(Error::Conflict(format!(
                "destination already tracked: {to}"
            )));
        }
        tree.remove(from);
        tree.insert(to.into(), revision);
        validate_path_set(tree.keys().map(String::as_str))?;
        self.check_destinations(&self.index_tree()?, &tree)?;
        let mut tx = Transaction::new("move");
        tx.files = vec![
            FileChange {
                path: from.into(),
                before: Some(expected.clone()),
                after: None,
            },
            FileChange {
                path: to.into(),
                before: None,
                after: Some(expected),
            },
        ];
        let id = self.save_tree(&tree)?;
        tx.metadata
            .push(self.meta_change("index", Some(pointer(&id)))?);
        let result = tx.files.clone();
        if !preview {
            self.execute(tx)?;
        }
        Ok(result)
    }
    pub fn remove(&self, path: &str, preview: bool) -> Result<Vec<FileChange>> {
        if self.backend.is_bare() {
            return Err(Error::Invalid("bare repository has no working tree".into()));
        }
        self.ensure_ready()?;
        validate_path(path)?;
        let mut tree = self.index_tree()?;
        let revision = tree
            .remove(path)
            .ok_or_else(|| Error::NotFound(path.into()))?;
        let state = self.state(&revision)?;
        if self.backend.file_state(path, state.executable)?.as_ref() != Some(&state)
            || self.head_tree()?.get(path) != Some(&revision)
        {
            return Err(Error::Conflict(format!(
                "{path} has staged or unstaged changes"
            )));
        }
        let id = self.save_tree(&tree)?;
        let mut tx = Transaction::new("remove");
        tx.files.push(FileChange {
            path: path.into(),
            before: Some(state),
            after: None,
        });
        tx.metadata
            .push(self.meta_change("index", Some(pointer(&id)))?);
        let result = tx.files.clone();
        if !preview {
            self.execute(tx)?;
        }
        Ok(result)
    }

    pub fn asset_at(&self, path: &str) -> Result<ObjectId> {
        let tree = self.index_tree()?;
        let id = tree.get(path).ok_or_else(|| Error::NotFound(path.into()))?;
        Ok(self.load::<Revision>(id)?.asset)
    }

    pub fn fsck(&self) -> Result<FsckReport> {
        let objects = self.backend.list_objects()?;
        for id in &objects {
            let kind = self.backend.object_kind(id)?;
            match kind {
                ObjectKind::Blob => {
                    self.backend.verify_blob(id)?;
                }
                ObjectKind::Asset => {
                    self.load::<Asset>(id)?;
                }
                ObjectKind::Revision => {
                    let r = self.load::<Revision>(id)?;
                    self.verify_revision_metadata(&r)?;
                    self.load::<Asset>(&r.asset)?;
                    self.backend.verify_blob(&r.blob)?;
                    for parent in r.parents {
                        if self.load::<Revision>(&parent)?.asset != r.asset {
                            return Err(Error::Integrity(
                                "revision parent belongs to another asset".into(),
                            ));
                        }
                    }
                }
                ObjectKind::Dependency => {
                    self.verify_dependency(&self.load::<Dependency>(id)?)?;
                }
                ObjectKind::Interface => {
                    self.verify_interface(&self.load::<Interface>(id)?)?;
                }
                ObjectKind::ValidationRecord => {
                    self.verify_validation_record(&self.load::<ValidationRecord>(id)?)?
                }
                ObjectKind::TestDefinition => {
                    let definition = self.load::<TestDefinition>(id)?;
                    self.load::<Asset>(&definition.procedure)?;
                    for input in definition.inputs {
                        self.load::<Asset>(&input)?;
                    }
                }
                ObjectKind::TraceLink => {
                    let link = self.load::<TraceLink>(id)?;
                    if self.load::<Revision>(&link.source_revision)?.asset != link.source
                        || self.load::<Revision>(&link.target_revision)?.asset != link.target
                    {
                        return Err(Error::Integrity("trace revision identity mismatch".into()));
                    }
                }
                ObjectKind::Release => {
                    let release = self.load::<Release>(id)?;
                    self.verify_release(&release)
                        .map_err(|e| Error::Integrity(e.to_string()))?;
                }
                ObjectKind::Snapshot => {
                    self.tree(id)?;
                    self.tests_in_snapshot(id)?;
                    for node in self.graph_snapshot(id)?.nodes {
                        if let Some(link) =
                            self.manifest_id(&node, ObjectKind::TraceLink, ".knot-link.json")?
                        {
                            self.load::<TraceLink>(&link)?;
                        }
                    }
                }
                ObjectKind::Commit => {
                    let c = self.load::<Commit>(id)?;
                    self.tree(&c.snapshot)?;
                    for id in c.overrides {
                        let record = self.load::<ValidationRecord>(&id)?;
                        self.verify_validation_record(&record)?;
                        if record.override_snapshot.is_none()
                            || !self.record_covers_snapshot(&record, &c.snapshot)?
                        {
                            return Err(Error::Integrity(
                                "commit override snapshot mismatch".into(),
                            ));
                        }
                    }
                    for parent in c.parents {
                        self.load::<Commit>(&parent)?;
                    }
                }
                ObjectKind::IssueEvent => {
                    let e = self.load::<IssueEvent>(id)?;
                    if let Some(parent) = e.parent
                        && self.load::<IssueEvent>(&parent)?.issue != e.issue
                    {
                        return Err(Error::Integrity(
                            "issue parent belongs to another issue".into(),
                        ));
                    }
                    if let IssueAction::Link { target } = e.action {
                        self.validate_issue_link(&target)?;
                    }
                }
            }
        }
        self.tree(&self.index_id()?)?;
        if let Some(head) = self.head_id()? {
            self.load::<Commit>(&head)?;
        }
        let refs = self.backend.list_meta("refs/")?;
        for name in &refs {
            let text = self
                .backend
                .read_meta(name)?
                .ok_or_else(|| Error::Integrity(format!("missing {name}")))?;
            let id = parse_pointer(&text)?;
            self.verify_reference(name, &id)?;
            if name.starts_with("refs/issues/") {
                let issue = self.load::<IssueEvent>(&id)?;
                if name != &format!("refs/issues/{}", issue.issue) {
                    return Err(Error::Integrity("issue ref identity mismatch".into()));
                }
                self.issue(&issue.issue.to_string())?;
            }
        }
        Ok(FsckReport {
            objects: objects.len(),
            refs: refs.len(),
            pending_transaction: self.backend.read_meta("transaction.json")?.is_some(),
        })
    }

    pub fn verify_reference(&self, name: &str, id: &ObjectId) -> Result<()> {
        if let Some(name) = name
            .strip_prefix("refs/heads/")
            .or_else(|| name.strip_prefix("refs/tags/"))
            .or_else(|| name.strip_prefix("refs/remotes/"))
        {
            validate_ref_name(name)?;
            self.load::<Commit>(id)?;
        } else if name.starts_with("refs/issues/") {
            if name != format!("refs/issues/{}", self.load::<IssueEvent>(id)?.issue) {
                return Err(Error::Integrity("issue ref identity mismatch".into()));
            }
        } else if name.starts_with("refs/reviews/") {
            let record = self.load::<ValidationRecord>(id)?;
            if review_ref(&record.subject)? != name {
                return Err(Error::Integrity("review ref subject mismatch".into()));
            }
        } else if let Some(name) = name.strip_prefix("refs/releases/") {
            if self.load::<Release>(id)?.name != name {
                return Err(Error::Integrity("release ref name mismatch".into()));
            }
        } else {
            return Err(Error::Integrity(format!(
                "unsupported ref namespace {name}"
            )));
        }
        Ok(())
    }
}

pub(crate) fn same_revision_state(a: &Revision, b: &Revision) -> bool {
    a.asset == b.asset
        && a.blob == b.blob
        && a.executable == b.executable
        && a.dependencies == b.dependencies
        && a.interfaces == b.interfaces
}
