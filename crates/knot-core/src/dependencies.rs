//! Versioned declarations and snapshot-based graph queries. No resolver or test runner.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub fn validate_interface_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        || !name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
    {
        return Err(Error::Invalid(
            "interface names must match [A-Za-z_][A-Za-z0-9_.-]{0,63}".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Interface {
    pub asset: ObjectId,
    pub name: String,
    /// Exact bytes of the declared contract, not inferred from the engineering file.
    pub definition: ObjectId,
}
impl StoredObject for Interface {
    const KIND: ObjectKind = ObjectKind::Interface;
    fn validate(&self) -> Result<()> {
        validate_interface_name(&self.name).map_err(|e| Error::Integrity(e.to_string()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DependencyRequirement {
    Revision { revision: ObjectId },
    Interface { name: String, interface: ObjectId },
}
impl DependencyRequirement {
    fn name(&self) -> Option<&str> {
        match self {
            Self::Revision { .. } => None,
            Self::Interface { name, .. } => Some(name),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub dependent: ObjectId,
    pub target: ObjectId,
    pub requirement: DependencyRequirement,
}
impl StoredObject for Dependency {
    const KIND: ObjectKind = ObjectKind::Dependency;
    fn validate(&self) -> Result<()> {
        if self.dependent == self.target {
            return Err(Error::Integrity("an asset cannot depend on itself".into()));
        }
        if let DependencyRequirement::Interface { name, .. } = &self.requirement {
            validate_interface_name(name).map_err(|e| Error::Integrity(e.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum DependencyRequest {
    Revision(Option<ObjectId>),
    Interface(String),
}
#[derive(Clone, Debug, Serialize)]
pub struct MetadataUpdate {
    pub snapshot: ObjectId,
    pub asset: ObjectId,
    pub revision: ObjectId,
    pub object: Option<ObjectId>,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct InterfaceExport {
    pub id: ObjectId,
    pub name: String,
    pub definition: ObjectId,
}
#[derive(Clone, Debug, Serialize)]
pub struct GraphNode {
    pub asset: ObjectId,
    pub path: String,
    pub revision: ObjectId,
    pub blob: ObjectId,
    pub executable: bool,
    pub dependencies: Vec<ObjectId>,
    pub interfaces: Vec<InterfaceExport>,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatus {
    Satisfied,
    MissingAsset,
    RevisionMismatch,
    MissingInterface,
    InterfaceMismatch,
}
#[derive(Clone, Debug, Serialize)]
pub struct DependencyEdge {
    pub id: ObjectId,
    pub dependent: ObjectId,
    pub dependent_path: String,
    pub target: ObjectId,
    pub target_path: Option<String>,
    pub requirement: DependencyRequirement,
    pub selected_revision: Option<ObjectId>,
    pub selected_interface: Option<ObjectId>,
    pub status: RequirementStatus,
}
#[derive(Clone, Debug, Serialize)]
pub struct GraphReport {
    pub snapshot: ObjectId,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<DependencyEdge>,
    pub requirements_satisfied: bool,
    pub declarations_only: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct DependenciesReport {
    pub declarations_only: bool,
    pub snapshot: ObjectId,
    pub root: GraphNode,
    pub reverse: bool,
    pub transitive: bool,
    pub edges: Vec<DependencyEdge>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ImpactEntry {
    pub asset: ObjectId,
    pub path: String,
    pub via: ObjectId,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ImpactReport {
    pub snapshot: ObjectId,
    pub baseline: Option<ObjectId>,
    pub mode: String,
    pub root: GraphNode,
    pub root_present: bool,
    pub changes: Vec<String>,
    pub direct: Vec<ImpactEntry>,
    pub transitive: Vec<ImpactEntry>,
    pub unaffected_direct: Vec<ImpactEntry>,
    pub declarations_only: bool,
}

impl<B: Backend> Repository<B> {
    pub(crate) fn verify_interface(&self, interface: &Interface) -> Result<()> {
        interface.validate()?;
        self.load::<Asset>(&interface.asset)?;
        self.backend.verify_blob(&interface.definition)?;
        Ok(())
    }
    pub(crate) fn verify_dependency(&self, dependency: &Dependency) -> Result<()> {
        dependency.validate()?;
        self.load::<Asset>(&dependency.dependent)?;
        self.load::<Asset>(&dependency.target)?;
        match &dependency.requirement {
            DependencyRequirement::Revision { revision } => {
                if self.load::<Revision>(revision)?.asset != dependency.target {
                    return Err(Error::Integrity(
                        "dependency pins a revision of another asset".into(),
                    ));
                }
            }
            DependencyRequirement::Interface { name, interface } => {
                let contract = self.load::<Interface>(interface)?;
                self.verify_interface(&contract)?;
                if contract.asset != dependency.target || contract.name != *name {
                    return Err(Error::Integrity(
                        "dependency pins another asset or interface name".into(),
                    ));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn verify_revision_metadata(&self, revision: &Revision) -> Result<()> {
        self.load::<Asset>(&revision.asset)?;
        let mut names = BTreeSet::new();
        for id in &revision.interfaces {
            let interface = self.load::<Interface>(id)?;
            self.verify_interface(&interface)?;
            if interface.asset != revision.asset || !names.insert(interface.name) {
                return Err(Error::Integrity(
                    "revision has duplicate or foreign interface exports".into(),
                ));
            }
        }
        let mut keys = BTreeSet::new();
        for id in &revision.dependencies {
            let dependency = self.load::<Dependency>(id)?;
            self.verify_dependency(&dependency)?;
            let key = (
                dependency.target.clone(),
                dependency.requirement.name().map(str::to_owned),
            );
            if dependency.dependent != revision.asset || !keys.insert(key) {
                return Err(Error::Integrity(
                    "revision has duplicate or foreign dependency requirements".into(),
                ));
            }
        }
        Ok(())
    }
    fn selected(&self, selector: &str, tree: &Tree) -> Result<(String, ObjectId, Revision)> {
        if let Some(id) = tree.get(selector) {
            return Ok((selector.into(), id.clone(), self.load::<Revision>(id)?));
        }
        if let Ok(asset) = selector.parse::<ObjectId>() {
            for (path, id) in tree {
                let revision = self.load::<Revision>(id)?;
                if revision.asset == asset {
                    return Ok((path.clone(), id.clone(), revision));
                }
            }
        }
        Err(Error::NotFound(format!(
            "selected asset {selector}; stage it with knot add first"
        )))
    }
    fn target_asset(&self, selector: &str, tree: &Tree) -> Result<ObjectId> {
        if let Ok((_, _, revision)) = self.selected(selector, tree) {
            return Ok(revision.asset);
        }
        if let Ok(id) = selector.parse::<ObjectId>() {
            self.load::<Asset>(&id)?;
            return Ok(id);
        }
        Ok(self.selected(selector, &self.head_tree()?)?.2.asset)
    }
    fn edit_revision(
        &self,
        mut tree: Tree,
        path: String,
        mut revision: Revision,
        object: Option<ObjectId>,
        purpose: &str,
    ) -> Result<MetadataUpdate> {
        revision.dependencies.sort();
        revision.dependencies.dedup();
        revision.interfaces.sort();
        revision.interfaces.dedup();
        self.verify_revision_metadata(&revision)?;
        let mut baseline = None;
        for id in self.head_tree()?.values() {
            let candidate = self.load::<Revision>(id)?;
            if candidate.asset == revision.asset {
                baseline = Some((id.clone(), candidate));
                break;
            }
        }
        let asset = revision.asset.clone();
        let id = if let Some((id, candidate)) = baseline {
            if same_revision_state(&revision, &candidate) {
                id
            } else {
                revision.parents = vec![id];
                self.put(&revision)?
            }
        } else {
            revision.parents.clear();
            self.put(&revision)?
        };
        tree.insert(path, id.clone());
        let snapshot = self.stage_tree(&tree, purpose)?;
        Ok(MetadataUpdate {
            snapshot,
            asset,
            revision: id,
            object,
        })
    }

    pub fn set_interface(
        &self,
        owner: &str,
        name: &str,
        definition_file: &str,
    ) -> Result<MetadataUpdate> {
        self.ensure_ready()?;
        validate_interface_name(name)?;
        let tree = self.index_tree()?;
        let (path, _, mut revision) = self.selected(owner, &tree)?;
        let definition = self
            .backend
            .store_file(definition_file, false)?
            .ok_or_else(|| Error::NotFound(definition_file.into()))?
            .blob;
        let interface = Interface {
            asset: revision.asset.clone(),
            name: name.into(),
            definition,
        };
        self.verify_interface(&interface)?;
        let id = self.put(&interface)?;
        let mut exports = Vec::new();
        for export in &revision.interfaces {
            if self.load::<Interface>(export)?.name != name {
                exports.push(export.clone());
            }
        }
        exports.push(id.clone());
        revision.interfaces = exports;
        self.edit_revision(tree, path, revision, Some(id), "interface set")
    }
    pub fn remove_interface(&self, owner: &str, name: &str) -> Result<MetadataUpdate> {
        self.ensure_ready()?;
        validate_interface_name(name)?;
        let tree = self.index_tree()?;
        let (path, _, mut revision) = self.selected(owner, &tree)?;
        let mut exports = Vec::new();
        let mut removed = false;
        for export in &revision.interfaces {
            if self.load::<Interface>(export)?.name == name {
                removed = true;
            } else {
                exports.push(export.clone());
            }
        }
        if !removed {
            return Err(Error::NotFound(format!("interface {name}")));
        }
        revision.interfaces = exports;
        self.edit_revision(tree, path, revision, None, "interface remove")
    }
    pub fn add_dependency(
        &self,
        owner: &str,
        target: &str,
        request: DependencyRequest,
    ) -> Result<MetadataUpdate> {
        self.ensure_ready()?;
        let tree = self.index_tree()?;
        let (path, _, mut revision) = self.selected(owner, &tree)?;
        let (_, target_revision, target_data) = self.selected(target, &tree)?;
        if revision.asset == target_data.asset {
            return Err(Error::Invalid("an asset cannot depend on itself".into()));
        }
        let requirement = match request {
            DependencyRequest::Revision(pin) => DependencyRequirement::Revision {
                revision: pin.unwrap_or(target_revision),
            },
            DependencyRequest::Interface(name) => {
                validate_interface_name(&name)?;
                let mut found = None;
                for export in &target_data.interfaces {
                    if self.load::<Interface>(export)?.name == name {
                        found = Some(export.clone());
                        break;
                    }
                }
                DependencyRequirement::Interface {
                    name: name.clone(),
                    interface: found.ok_or_else(|| {
                        Error::NotFound(format!("exported interface {name} on {target}"))
                    })?,
                }
            }
        };
        let dependency = Dependency {
            dependent: revision.asset.clone(),
            target: target_data.asset,
            requirement,
        };
        self.verify_dependency(&dependency)?;
        let id = self.put(&dependency)?;
        let mut requirements = Vec::new();
        for existing in &revision.dependencies {
            let old = self.load::<Dependency>(existing)?;
            if old.target != dependency.target
                || old.requirement.name() != dependency.requirement.name()
            {
                requirements.push(existing.clone());
            }
        }
        requirements.push(id.clone());
        revision.dependencies = requirements;
        self.edit_revision(tree, path, revision, Some(id), "depend add")
    }
    pub fn remove_dependency(
        &self,
        owner: &str,
        target: &str,
        interface: Option<&str>,
    ) -> Result<MetadataUpdate> {
        self.ensure_ready()?;
        if let Some(name) = interface {
            validate_interface_name(name)?;
        }
        let tree = self.index_tree()?;
        let (path, _, mut revision) = self.selected(owner, &tree)?;
        let target = self.target_asset(target, &tree)?;
        let mut requirements = Vec::new();
        let mut removed = false;
        for existing in &revision.dependencies {
            let old = self.load::<Dependency>(existing)?;
            if old.target == target
                && interface.is_none_or(|name| old.requirement.name() == Some(name))
            {
                removed = true;
            } else {
                requirements.push(existing.clone());
            }
        }
        if !removed {
            return Err(Error::NotFound("matching dependency".into()));
        }
        revision.dependencies = requirements;
        self.edit_revision(tree, path, revision, None, "depend remove")
    }
    pub fn graph(&self, at: Option<&str>) -> Result<GraphReport> {
        let snapshot = if let Some(at) = at {
            self.load::<Commit>(&self.resolve(at)?)?.snapshot
        } else {
            self.index_id()?
        };
        self.graph_snapshot(&snapshot)
    }
    pub(crate) fn graph_snapshot(&self, snapshot: &ObjectId) -> Result<GraphReport> {
        let tree = self.tree(snapshot)?;
        let mut nodes = Vec::new();
        for (path, id) in tree {
            let revision = self.load::<Revision>(&id)?;
            let mut interfaces = Vec::new();
            for export in &revision.interfaces {
                let interface = self.load::<Interface>(export)?;
                interfaces.push(InterfaceExport {
                    id: export.clone(),
                    name: interface.name,
                    definition: interface.definition,
                });
            }
            interfaces.sort_by(|a, b| a.name.cmp(&b.name));
            nodes.push(GraphNode {
                asset: revision.asset,
                path,
                revision: id,
                blob: revision.blob,
                executable: revision.executable,
                dependencies: revision.dependencies,
                interfaces,
            });
        }
        let by_asset: BTreeMap<_, _> = nodes.iter().map(|n| (&n.asset, n)).collect();
        let mut edges = Vec::new();
        for node in &nodes {
            for id in &node.dependencies {
                let dependency = self.load::<Dependency>(id)?;
                let selected = by_asset.get(&dependency.target);
                let mut selected_interface = None;
                let status = if let Some(selected) = selected {
                    match &dependency.requirement {
                        DependencyRequirement::Revision { revision } => {
                            if selected.revision == *revision {
                                RequirementStatus::Satisfied
                            } else {
                                RequirementStatus::RevisionMismatch
                            }
                        }
                        DependencyRequirement::Interface { name, interface } => {
                            selected_interface = selected
                                .interfaces
                                .iter()
                                .find(|i| i.name == *name)
                                .map(|i| i.id.clone());
                            match &selected_interface {
                                None => RequirementStatus::MissingInterface,
                                Some(id) if id == interface => RequirementStatus::Satisfied,
                                _ => RequirementStatus::InterfaceMismatch,
                            }
                        }
                    }
                } else {
                    RequirementStatus::MissingAsset
                };
                edges.push(DependencyEdge {
                    id: id.clone(),
                    dependent: dependency.dependent,
                    dependent_path: node.path.clone(),
                    target: dependency.target,
                    target_path: selected.map(|n| n.path.clone()),
                    requirement: dependency.requirement,
                    selected_revision: selected.map(|n| n.revision.clone()),
                    selected_interface,
                    status,
                });
            }
        }
        let requirements_satisfied = edges
            .iter()
            .all(|e| e.status == RequirementStatus::Satisfied);
        Ok(GraphReport {
            snapshot: snapshot.clone(),
            nodes,
            edges,
            requirements_satisfied,
            declarations_only: true,
        })
    }
    pub(crate) fn require_satisfied(&self, snapshot: &ObjectId) -> Result<()> {
        let conflicts: Vec<_> = self
            .graph_snapshot(snapshot)?
            .edges
            .into_iter()
            .filter(|e| e.status != RequirementStatus::Satisfied)
            .collect();
        if !conflicts.is_empty() {
            return Err(Error::UnsatisfiedDependencies(conflicts));
        }
        Ok(())
    }
    pub fn dependencies(
        &self,
        selector: &str,
        at: Option<&str>,
        reverse: bool,
        transitive: bool,
    ) -> Result<DependenciesReport> {
        let graph = self.graph(at)?;
        let root = graph_node(&graph, selector)?.clone();
        let mut queue = VecDeque::from([root.asset.clone()]);
        let mut seen = BTreeSet::from([root.asset.clone()]);
        let mut edge_ids = BTreeSet::new();
        while let Some(asset) = queue.pop_front() {
            for edge in &graph.edges {
                let (from, to) = if reverse {
                    (&edge.target, &edge.dependent)
                } else {
                    (&edge.dependent, &edge.target)
                };
                if *from == asset {
                    edge_ids.insert(edge.id.clone());
                    if transitive && seen.insert(to.clone()) {
                        queue.push_back(to.clone());
                    }
                }
            }
        }
        Ok(DependenciesReport {
            declarations_only: true,
            snapshot: graph.snapshot,
            root,
            reverse,
            transitive,
            edges: graph
                .edges
                .into_iter()
                .filter(|e| edge_ids.contains(&e.id))
                .collect(),
        })
    }
    pub fn impact(
        &self,
        selector: &str,
        at: Option<&str>,
        since: Option<&str>,
    ) -> Result<ImpactReport> {
        let graph = self.graph(at)?;
        let baseline = since
            .map(|reference| self.graph(Some(reference)))
            .transpose()?;
        let root = graph_node(&graph, selector)
            .or_else(|e| {
                baseline
                    .as_ref()
                    .map_or(Err(e), |b| graph_node(b, selector))
            })?
            .clone();
        let before = baseline
            .as_ref()
            .and_then(|b| b.nodes.iter().find(|n| n.asset == root.asset));
        let after = graph.nodes.iter().find(|n| n.asset == root.asset);
        let mut changes = Vec::new();
        if baseline.is_some() {
            match (before, after) {
                (None, Some(_)) => changes.push("added".into()),
                (Some(_), None) => changes.push("removed".into()),
                (Some(a), Some(b)) => {
                    if a.blob != b.blob || a.executable != b.executable {
                        changes.push("content".into());
                    }
                    if a.interfaces != b.interfaces {
                        changes.push("interfaces".into());
                    }
                    if a.dependencies != b.dependencies {
                        changes.push("constraints".into());
                    }
                    if a.path != b.path {
                        changes.push("path".into());
                    }
                    if a.revision != b.revision && changes.is_empty() {
                        changes.push("revision".into());
                    }
                }
                _ => {}
            }
        }
        let mut direct = Vec::new();
        let mut transitive = Vec::new();
        let mut unaffected_direct = Vec::new();
        let mut queue = VecDeque::from([(root.asset.clone(), 0usize)]);
        let mut seen = BTreeSet::from([root.asset.clone()]);
        while let Some((asset, depth)) = queue.pop_front() {
            for edge in graph.edges.iter().filter(|e| e.target == asset) {
                let node = graph
                    .nodes
                    .iter()
                    .find(|n| n.asset == edge.dependent)
                    .ok_or_else(|| Error::Integrity("missing dependent node".into()))?;
                let affects = if baseline.is_none() || depth > 0 {
                    true
                } else {
                    match &edge.requirement {
                        DependencyRequirement::Revision { .. } => {
                            before.map(|n| &n.revision) != after.map(|n| &n.revision)
                        }
                        DependencyRequirement::Interface { name, .. } => {
                            export_id(before, name) != export_id(after, name)
                        }
                    }
                };
                let entry = ImpactEntry {
                    asset: node.asset.clone(),
                    path: node.path.clone(),
                    via: asset.clone(),
                    reason: if !affects {
                        "required_contract_unchanged"
                    } else if depth > 0 {
                        "transitive_dependency_change"
                    } else if baseline.is_none() {
                        "potential_dependency_change"
                    } else {
                        "required_revision_or_interface_changed"
                    }
                    .into(),
                };
                if !affects {
                    unaffected_direct.push(entry);
                    continue;
                }
                if seen.insert(node.asset.clone()) {
                    if depth == 0 {
                        direct.push(entry);
                    } else {
                        transitive.push(entry);
                    }
                    queue.push_back((node.asset.clone(), depth + 1));
                }
            }
        }
        unaffected_direct.retain(|e| !seen.contains(&e.asset));
        for entries in [&mut direct, &mut transitive, &mut unaffected_direct] {
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            entries.dedup_by(|a, b| a.asset == b.asset);
        }
        Ok(ImpactReport {
            snapshot: graph.snapshot,
            baseline: baseline.map(|b| b.snapshot),
            mode: if since.is_some() {
                "changes"
            } else {
                "potential"
            }
            .into(),
            root_present: after.is_some(),
            root,
            changes,
            direct,
            transitive,
            unaffected_direct,
            declarations_only: true,
        })
    }
}
fn graph_node<'a>(graph: &'a GraphReport, selector: &str) -> Result<&'a GraphNode> {
    graph
        .nodes
        .iter()
        .find(|n| n.path == selector || n.asset.as_str() == selector)
        .ok_or_else(|| Error::NotFound(format!("asset {selector} in selected snapshot")))
}
fn export_id<'a>(node: Option<&'a GraphNode>, name: &str) -> Option<&'a ObjectId> {
    node.and_then(|n| n.interfaces.iter().find(|i| i.name == name))
        .map(|i| &i.id)
}
