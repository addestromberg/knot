//! Deterministic, bounded constraint resolution over immutable revision history.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResolutionPin {
    pub asset: String,
    pub revision: ObjectId,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionPlan {
    pub expected_head: Option<ObjectId>,
    pub expected_index: ObjectId,
    pub pins: Vec<ResolutionPin>,
    pub snapshot: Snapshot,
    pub snapshot_id: ObjectId,
    pub changes: Vec<FileChange>,
    pub policy: String,
    pub validated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolutionConflict {
    pub dependent: ObjectId,
    pub target: ObjectId,
    pub requirement: DependencyRequirement,
    pub selected: Option<ObjectId>,
    pub reason: String,
}

impl<B: Backend> Repository<B> {
    pub fn revision_descends_from(&self, revision: &ObjectId, ancestor: &ObjectId) -> Result<bool> {
        let mut pending = vec![revision.clone()];
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if id == *ancestor {
                return Ok(true);
            }
            if seen.insert(id.clone()) {
                pending.extend(self.load::<Revision>(&id)?.parents);
            }
        }
        Ok(false)
    }

    /// Candidate history excludes unrelated orphan objects left by abandoned staging.
    fn candidate_revisions(&self) -> Result<BTreeMap<ObjectId, Revision>> {
        let mut commits = self.head_id()?.into_iter().collect::<Vec<_>>();
        for name in self.backend.list_meta("refs/")? {
            if name.starts_with("refs/heads/")
                || name.starts_with("refs/tags/")
                || name.starts_with("refs/remotes/")
            {
                commits.push(parse_pointer(
                    &self
                        .backend
                        .read_meta(&name)?
                        .ok_or_else(|| Error::Integrity("missing ref".into()))?,
                )?);
            }
        }
        let mut revisions: Vec<_> = self.index_tree()?.into_values().collect();
        let mut seen = BTreeSet::new();
        while let Some(id) = commits.pop() {
            if seen.insert(id.clone()) {
                let commit = self.load::<Commit>(&id)?;
                revisions.extend(self.tree(&commit.snapshot)?.into_values());
                commits.extend(commit.parents);
            }
        }
        let mut found = BTreeMap::new();
        while let Some(id) = revisions.pop() {
            if let std::collections::btree_map::Entry::Vacant(entry) = found.entry(id.clone()) {
                let revision = self.load::<Revision>(&id)?;
                revisions.extend(revision.parents.clone());
                for dependency in &revision.dependencies {
                    match self.load::<Dependency>(dependency)?.requirement {
                        DependencyRequirement::Revision { revision } => revisions.push(revision),
                        DependencyRequirement::RevisionRange { minimum, maximum } => {
                            revisions.push(minimum);
                            revisions.extend(maximum);
                        }
                        DependencyRequirement::Interface { .. } => {}
                    }
                }
                entry.insert(revision);
            }
        }
        Ok(found)
    }

    pub fn plan_resolution(&self, pins: &[ResolutionPin]) -> Result<ResolutionPlan> {
        self.ensure_ready()?;
        let current = self.index_tree()?;
        let mut paths = BTreeMap::new();
        for (path, id) in &current {
            paths.insert(self.load::<Revision>(id)?.asset, path.clone());
        }
        let mut candidates = self.candidate_revisions()?;
        let mut pinned = BTreeMap::new();
        for pin in pins {
            let asset = if let Some(id) = current.get(&pin.asset) {
                self.load::<Revision>(id)?.asset
            } else {
                pin.asset.parse::<ObjectId>()?
            };
            if !paths.contains_key(&asset) {
                return Err(Error::NotFound(format!("selected asset {}", pin.asset)));
            }
            let revision = self.load::<Revision>(&pin.revision)?;
            self.verify_revision_metadata(&revision)?;
            if revision.asset != asset {
                return Err(Error::Invalid("pin belongs to another asset".into()));
            }
            if let Some(old) = pinned.insert(asset, pin.revision.clone())
                && old != pin.revision
            {
                return Err(Error::Conflict("conflicting explicit pins".into()));
            }
            candidates.insert(pin.revision.clone(), revision);
        }
        let mut by_asset: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
        let mut required_keys = BTreeMap::new();
        for id in current.values() {
            let revision = self.load::<Revision>(id)?;
            let keys: BTreeSet<_> = revision
                .dependencies
                .iter()
                .map(|id| {
                    let dependency = self.load::<Dependency>(id)?;
                    Ok((
                        dependency.target,
                        dependency.requirement.name().map(str::to_owned),
                    ))
                })
                .collect::<Result<_>>()?;
            required_keys.insert(revision.asset, keys);
        }
        for (id, revision) in &candidates {
            if paths.contains_key(&revision.asset) {
                let keys: BTreeSet<_> = revision
                    .dependencies
                    .iter()
                    .map(|id| {
                        let dependency = self.load::<Dependency>(id)?;
                        Ok((
                            dependency.target,
                            dependency.requirement.name().map(str::to_owned),
                        ))
                    })
                    .collect::<Result<_>>()?;
                if !required_keys[&revision.asset].is_subset(&keys) {
                    continue;
                }
                by_asset
                    .entry(revision.asset.clone())
                    .or_default()
                    .push(id.clone());
            }
        }
        for (asset, ids) in &mut by_asset {
            let mut ranked = Vec::new();
            for id in ids.iter() {
                let mut seen = BTreeSet::new();
                let mut pending = vec![id.clone()];
                while let Some(id) = pending.pop() {
                    if seen.insert(id.clone()) {
                        pending.extend(self.load::<Revision>(&id)?.parents);
                    }
                }
                ranked.push((seen.len(), id.clone()));
            }
            ranked.sort_by(|a, b| b.cmp(a));
            *ids = ranked
                .into_iter()
                .map(|(_, id)| id)
                .filter(|id| pinned.get(asset).is_none_or(|pin| pin == id))
                .collect();
        }
        let graph = self.graph(None)?;
        let targets: BTreeSet<_> = graph.edges.iter().map(|e| &e.target).collect();
        let mut order = Vec::new();
        let mut visited = BTreeSet::new();
        for root in graph
            .nodes
            .iter()
            .filter(|n| !targets.contains(&n.asset))
            .chain(graph.nodes.iter())
        {
            let mut pending = vec![root.asset.clone()];
            while let Some(asset) = pending.pop() {
                if !paths.contains_key(&asset) || !visited.insert(asset.clone()) {
                    continue;
                }
                order.push(asset.clone());
                let mut children: Vec<_> = graph
                    .edges
                    .iter()
                    .filter(|e| e.dependent == asset)
                    .map(|e| e.target.clone())
                    .collect();
                children.sort_by_key(|id| paths.get(id));
                pending.extend(children.into_iter().rev());
            }
        }
        let dependencies: BTreeMap<_, _> = candidates
            .values()
            .flat_map(|r| &r.dependencies)
            .map(|id| Ok((id.clone(), self.load::<Dependency>(id)?)))
            .collect::<Result<_>>()?;
        let mut positions = vec![0usize; order.len()];
        let mut selected = BTreeMap::new();
        let mut depth = 0usize;
        let mut attempts = 0usize;
        let mut conflicts = Vec::new();
        loop {
            if depth == order.len() {
                break;
            }
            let asset = &order[depth];
            let choices = &by_asset[asset];
            if positions[depth] >= choices.len() {
                positions[depth] = 0;
                selected.remove(asset);
                if depth == 0 {
                    return Err(Error::ResolutionConflict {
                        conflicts,
                        exhausted: false,
                    });
                }
                depth -= 1;
                continue;
            }
            let id = choices[positions[depth]].clone();
            positions[depth] += 1;
            selected.insert(asset.clone(), id);
            attempts += 1;
            if attempts > 100_000 {
                return Err(Error::ResolutionConflict {
                    conflicts,
                    exhausted: true,
                });
            }
            let mut bad = Vec::new();
            for revision_id in selected.values() {
                let revision = &candidates[revision_id];
                for dependency_id in &revision.dependencies {
                    let dependency = &dependencies[dependency_id];
                    let target = selected.get(&dependency.target);
                    let missing = !paths.contains_key(&dependency.target);
                    let compatible = if let Some(target) = target {
                        let data = &candidates[target];
                        match &dependency.requirement {
                            DependencyRequirement::Revision { revision } => target == revision,
                            DependencyRequirement::RevisionRange { minimum, maximum } => {
                                self.revision_descends_from(target, minimum)?
                                    && maximum
                                        .as_ref()
                                        .map(|max| self.revision_descends_from(max, target))
                                        .transpose()?
                                        .unwrap_or(true)
                            }
                            DependencyRequirement::Interface { interface, .. } => {
                                data.interfaces.contains(interface)
                            }
                        }
                    } else {
                        !missing
                    };
                    if !compatible {
                        bad.push(ResolutionConflict {
                            dependent: dependency.dependent.clone(),
                            target: dependency.target.clone(),
                            requirement: dependency.requirement.clone(),
                            selected: target.cloned(),
                            reason: if missing {
                                "target absent from selected project"
                            } else {
                                "selected revision does not satisfy requirement"
                            }
                            .into(),
                        });
                    }
                }
            }
            if bad.is_empty() {
                depth += 1;
            } else {
                conflicts = bad;
            }
        }
        let tree: Tree = paths
            .into_iter()
            .map(|(asset, path)| {
                Ok((
                    path,
                    selected
                        .get(&asset)
                        .cloned()
                        .ok_or_else(|| Error::Integrity("resolver omitted asset".into()))?,
                ))
            })
            .collect::<Result<_>>()?;
        let snapshot = Snapshot {
            entries: tree
                .iter()
                .map(|(path, revision)| SnapshotEntry {
                    path: path.clone(),
                    revision: revision.clone(),
                })
                .collect(),
        };
        let snapshot_id = object_id(ObjectKind::Snapshot, &canonical(&snapshot)?);
        Ok(ResolutionPlan { expected_head:self.head_id()?, expected_index:self.index_id()?, pins:pins.to_vec(), snapshot, snapshot_id, changes:self.tree_file_changes(&current,&tree)?, policy:"dependency roots then path order; newest by ancestor count then descending hash; preserve current relationships and explicit pins".into(), validated:false })
    }

    pub fn apply_resolution(&self, plan: &ResolutionPlan) -> Result<()> {
        if self.backend.is_bare() {
            return Err(Error::Invalid("bare repository has no working tree".into()));
        }
        self.ensure_ready()?;
        let status = self.status()?;
        if !status.staged.is_empty() || !status.unstaged.is_empty() {
            return Err(Error::Conflict(
                "resolved checkout requires clean staging and tracked working files".into(),
            ));
        }
        if status.head != plan.expected_head || status.staged_snapshot != plan.expected_index {
            return Err(Error::Conflict("resolution preview is stale".into()));
        }
        let fresh = self.plan_resolution(&plan.pins)?;
        if fresh.snapshot != plan.snapshot
            || fresh.changes != plan.changes
            || fresh.snapshot_id != plan.snapshot_id
        {
            return Err(Error::Conflict(
                "resolution result changed; preview again".into(),
            ));
        }
        let before = self.index_tree()?;
        let after: Tree = plan
            .snapshot
            .entries
            .iter()
            .map(|e| (e.path.clone(), e.revision.clone()))
            .collect();
        self.check_destinations(&before, &after)?;
        let snapshot = self.save_tree(&after)?;
        self.require_satisfied(&snapshot)?;
        let mut tx = Transaction::new("resolved checkout");
        tx.files = self.tree_file_changes(&before, &after)?;
        tx.metadata
            .push(self.meta_change("index", Some(pointer(&snapshot)))?);
        self.execute(tx)
    }
}
