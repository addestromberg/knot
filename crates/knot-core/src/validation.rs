//! Manual exact-input evidence, versioned test/trace manifests, and gated releases.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewSubject {
    Asset {
        asset: ObjectId,
    },
    Dependency {
        dependent: ObjectId,
        target: ObjectId,
    },
    Test {
        definition: ObjectId,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewOutcome {
    Passed,
    Failed,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ValidationRecord {
    pub subject: ReviewSubject,
    pub parent: Option<ObjectId>,
    pub inputs: BTreeMap<ObjectId, ObjectId>,
    pub requirements: Vec<ObjectId>,
    pub outcome: ReviewOutcome,
    pub reviewer: String,
    pub started: String,
    pub finished: String,
    pub environment: String,
    pub notes: String,
    pub artifacts: Vec<ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_snapshot: Option<ObjectId>,
}
impl StoredObject for ValidationRecord {
    const KIND: ObjectKind = ObjectKind::ValidationRecord;
    fn validate(&self) -> Result<()> {
        validate_timestamp(&self.started)?;
        validate_timestamp(&self.finished)?;
        if self.started.parse::<u64>().ok() > self.finished.parse::<u64>().ok()
            || self.reviewer.trim().is_empty()
            || self.notes.trim().is_empty()
            || self.inputs.is_empty()
            || self.environment.trim().is_empty()
        {
            return Err(Error::Integrity(
                "review requires inputs, reviewer, notes, environment, and ordered timestamps"
                    .into(),
            ));
        }
        if self.requirements.windows(2).any(|w| w[0] >= w[1]) {
            return Err(Error::Integrity(
                "review requirements must be sorted and unique".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestDefinition {
    pub name: String,
    pub procedure: ObjectId,
    pub inputs: Vec<ObjectId>,
    pub description: String,
}
impl StoredObject for TestDefinition {
    const KIND: ObjectKind = ObjectKind::TestDefinition;
    fn validate(&self) -> Result<()> {
        validate_ref_name(&self.name)?;
        if self.inputs.windows(2).any(|w| w[0] >= w[1]) || self.inputs.is_empty() {
            return Err(Error::Integrity(
                "test inputs must be nonempty, sorted, and unique".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TraceLink {
    pub source: ObjectId,
    pub source_revision: ObjectId,
    pub target: ObjectId,
    pub target_revision: ObjectId,
    pub relation: String,
}
impl StoredObject for TraceLink {
    const KIND: ObjectKind = ObjectKind::TraceLink;
    fn validate(&self) -> Result<()> {
        if self.source == self.target
            || !["documents", "satisfies", "verifies", "describes"]
                .contains(&self.relation.as_str())
        {
            return Err(Error::Integrity("invalid trace relationship".into()));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ReleasePolicy {
    pub require_dependency_reviews: bool,
    pub require_all_tests: bool,
    pub required_tests: Vec<String>,
    pub required_assets: Vec<ObjectId>,
}
impl Default for ReleasePolicy {
    fn default() -> Self {
        Self {
            require_dependency_reviews: true,
            require_all_tests: true,
            required_tests: vec![],
            required_assets: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub name: String,
    pub commit: ObjectId,
    pub snapshot: ObjectId,
    pub policy: ReleasePolicy,
    pub evidence: Vec<ObjectId>,
    pub approver: String,
    pub timestamp: String,
    pub notes: String,
}
impl StoredObject for Release {
    const KIND: ObjectKind = ObjectKind::Release;
    fn validate(&self) -> Result<()> {
        validate_ref_name(&self.name)?;
        validate_timestamp(&self.timestamp)?;
        if self.approver.trim().is_empty() {
            return Err(Error::Integrity("release requires an approver".into()));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Unverified,
    Verified,
    Stale,
    Failed,
    Error,
}
#[derive(Clone, Debug, Serialize)]
pub struct VerificationEntry {
    pub subject: ReviewSubject,
    pub path: String,
    pub target_path: Option<String>,
    pub state: VerificationState,
    pub record: Option<ObjectId>,
    pub reason: String,
    pub overridden: bool,
}
pub struct ReviewDetails {
    pub outcome: ReviewOutcome,
    pub reviewer: String,
    pub notes: String,
    pub environment: String,
    pub artifacts: Vec<String>,
    pub overridden: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct UpstreamWarning {
    pub asset: ObjectId,
    pub path: String,
    pub via: ObjectId,
    pub dependent: ObjectId,
    pub target: ObjectId,
}
#[derive(Clone, Debug, Serialize)]
pub struct VerificationReport {
    pub snapshot: ObjectId,
    pub dependencies: Vec<VerificationEntry>,
    pub tests: Vec<VerificationEntry>,
    pub assets: Vec<VerificationEntry>,
    pub upstream: Vec<UpstreamWarning>,
}
#[derive(Clone, Debug, Serialize)]
pub struct NamedTest {
    pub asset: ObjectId,
    pub path: String,
    pub object: ObjectId,
    pub definition: TestDefinition,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    kind: String,
    object: ObjectId,
}

pub fn review_ref(subject: &ReviewSubject) -> Result<String> {
    Ok(format!(
        "refs/reviews/{}",
        object_id(ObjectKind::ValidationRecord, &canonical(subject)?)
    ))
}

impl<B: Backend> Repository<B> {
    pub fn snapshot_at(&self, at: Option<&str>) -> Result<ObjectId> {
        if let Some(at) = at {
            Ok(self.load::<Commit>(&self.resolve(at)?)?.snapshot)
        } else {
            self.index_id()
        }
    }
    fn node_for<'a>(&self, graph: &'a GraphReport, selector: &str) -> Result<&'a GraphNode> {
        graph
            .nodes
            .iter()
            .find(|n| n.path == selector || n.asset.as_str() == selector)
            .ok_or_else(|| Error::NotFound(format!("asset {selector}")))
    }
    pub fn review_subject(&self, asset: &str, on: Option<&str>) -> Result<ReviewSubject> {
        let graph = self.graph(None)?;
        let owner = self.node_for(&graph, asset)?.asset.clone();
        Ok(if let Some(on) = on {
            ReviewSubject::Dependency {
                dependent: owner,
                target: self.node_for(&graph, on)?.asset.clone(),
            }
        } else {
            ReviewSubject::Asset { asset: owner }
        })
    }
    pub(crate) fn validate_versioned_manifest(
        &self,
        path: &str,
        revision: &Revision,
    ) -> Result<Option<String>> {
        let kind = if path.ends_with(".knot-test.json") {
            ObjectKind::TestDefinition
        } else if path.ends_with(".knot-link.json") {
            ObjectKind::TraceLink
        } else {
            return Ok(None);
        };
        let (_, bytes) = self.backend.get_object(&revision.blob)?;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        if manifest.kind != kind.as_str() {
            return Err(Error::Integrity(
                "manifest kind does not match its reserved suffix".into(),
            ));
        }
        if kind == ObjectKind::TestDefinition {
            let definition = self.load::<TestDefinition>(&manifest.object)?;
            self.load::<Asset>(&definition.procedure)?;
            for input in definition.inputs {
                self.load::<Asset>(&input)?;
            }
            Ok(Some(definition.name))
        } else {
            let link = self.load::<TraceLink>(&manifest.object)?;
            if self.load::<Revision>(&link.source_revision)?.asset != link.source
                || self.load::<Revision>(&link.target_revision)?.asset != link.target
            {
                return Err(Error::Integrity("trace revision identity mismatch".into()));
            }
            Ok(None)
        }
    }

    pub(crate) fn manifest_id(
        &self,
        node: &GraphNode,
        kind: ObjectKind,
        suffix: &str,
    ) -> Result<Option<ObjectId>> {
        if !node.path.ends_with(suffix) {
            return Ok(None);
        }
        let (_, bytes) = self.backend.get_object(&node.blob)?;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        if manifest.kind != kind.as_str() {
            return Err(Error::Integrity(format!("invalid manifest {}", node.path)));
        }
        Ok(Some(manifest.object))
    }
    pub fn tests_in_snapshot(&self, snapshot: &ObjectId) -> Result<Vec<NamedTest>> {
        let graph = self.checked_graph(snapshot)?;
        let mut out = Vec::new();
        let mut names = BTreeSet::new();
        for node in &graph.nodes {
            if let Some(id) =
                self.manifest_id(node, ObjectKind::TestDefinition, ".knot-test.json")?
            {
                let definition = self.load::<TestDefinition>(&id)?;
                self.load::<Asset>(&definition.procedure)?;
                for input in &definition.inputs {
                    self.load::<Asset>(input)?;
                }
                if !names.insert(definition.name.clone()) {
                    return Err(Error::Integrity("duplicate test name".into()));
                }
                out.push(NamedTest {
                    asset: node.asset.clone(),
                    path: node.path.clone(),
                    object: id,
                    definition,
                });
            }
        }
        Ok(out)
    }
    fn review_inputs(
        &self,
        graph: &GraphReport,
        subject: &ReviewSubject,
    ) -> Result<(BTreeMap<ObjectId, ObjectId>, Vec<ObjectId>)> {
        let mut assets = Vec::new();
        let mut requirements = Vec::new();
        match subject {
            ReviewSubject::Asset { asset } => assets.push(asset.clone()),
            ReviewSubject::Dependency { dependent, target } => {
                assets.extend([dependent.clone(), target.clone()]);
                requirements = graph
                    .edges
                    .iter()
                    .filter(|e| e.dependent == *dependent && e.target == *target)
                    .map(|e| e.id.clone())
                    .collect();
                if requirements.is_empty() {
                    return Err(Error::NotFound("dependency relationship".into()));
                }
            }
            ReviewSubject::Test { definition } => {
                let test = self
                    .tests_in_snapshot(&graph.snapshot)?
                    .into_iter()
                    .find(|t| t.asset == *definition)
                    .ok_or_else(|| Error::NotFound("test definition".into()))?;
                assets.push(definition.clone());
                assets.push(test.definition.procedure);
                assets.extend(test.definition.inputs);
            }
        }
        let mut inputs = BTreeMap::new();
        for asset in assets {
            let node = graph
                .nodes
                .iter()
                .find(|n| n.asset == asset)
                .ok_or_else(|| Error::NotFound(format!("review input {asset} absent")))?;
            inputs.insert(asset, node.revision.clone());
        }
        requirements.sort();
        requirements.dedup();
        Ok((inputs, requirements))
    }
    pub fn verify_validation_record(&self, record: &ValidationRecord) -> Result<()> {
        let _reads = self.read_scope();
        record.validate()?;
        for (asset, revision) in &record.inputs {
            if self.load::<Revision>(revision)?.asset != *asset {
                return Err(Error::Integrity("review input identity mismatch".into()));
            }
        }
        if record.override_snapshot.is_some()
            && (record.outcome != ReviewOutcome::Passed
                || !matches!(record.subject, ReviewSubject::Dependency { .. }))
        {
            return Err(Error::Integrity(
                "only accepted dependency overrides may bind an override snapshot".into(),
            ));
        }
        for id in &record.requirements {
            self.verify_dependency(&self.load::<Dependency>(id)?)?;
        }
        for id in &record.artifacts {
            self.checked_blob(id)?;
        }
        if let Some(parent) = &record.parent
            && self.load::<ValidationRecord>(parent)?.subject != record.subject
        {
            return Err(Error::Integrity(
                "review parent has a different subject".into(),
            ));
        }
        let required: Vec<_> = match &record.subject {
            ReviewSubject::Asset { asset } => vec![asset],
            ReviewSubject::Dependency { dependent, target } => vec![dependent, target],
            ReviewSubject::Test { definition } => vec![definition],
        };
        if required
            .into_iter()
            .any(|id| !record.inputs.contains_key(id))
        {
            return Err(Error::Integrity("review omitted subject inputs".into()));
        }
        match &record.subject {
            ReviewSubject::Asset { .. } => {
                if record.inputs.len() != 1 || !record.requirements.is_empty() {
                    return Err(Error::Integrity(
                        "asset review has unexpected inputs".into(),
                    ));
                }
            }
            ReviewSubject::Dependency { dependent, target } => {
                let owner = self.load::<Revision>(&record.inputs[dependent])?;
                let selected = self.load::<Revision>(&record.inputs[target])?;
                let mut requirements = Vec::new();
                for id in owner.dependencies {
                    let dependency = self.load::<Dependency>(&id)?;
                    if dependency.target != *target {
                        continue;
                    }
                    if record.outcome == ReviewOutcome::Passed {
                        let matches = match dependency.requirement {
                            DependencyRequirement::Revision { revision } => {
                                revision == record.inputs[target]
                            }
                            DependencyRequirement::Interface { interface, .. } => {
                                selected.interfaces.contains(&interface)
                            }
                            DependencyRequirement::RevisionRange { minimum, maximum } => {
                                self.revision_descends_from(&record.inputs[target], &minimum)?
                                    && maximum
                                        .as_ref()
                                        .map(|maximum| {
                                            self.revision_descends_from(
                                                maximum,
                                                &record.inputs[target],
                                            )
                                        })
                                        .transpose()?
                                        .unwrap_or(true)
                            }
                        };
                        if !matches {
                            return Err(Error::Integrity(
                                "passed dependency review has unsatisfied requirements".into(),
                            ));
                        }
                    }
                    requirements.push(id);
                }
                requirements.sort();
                if record.inputs.len() != 2
                    || requirements.is_empty()
                    || requirements != record.requirements
                {
                    return Err(Error::Integrity(
                        "dependency review omitted or altered requirements".into(),
                    ));
                }
            }
            ReviewSubject::Test { definition } => {
                let revision = self.load::<Revision>(&record.inputs[definition])?;
                let (_, bytes) = self.backend.get_object(&revision.blob)?;
                let manifest: Manifest = serde_json::from_slice(&bytes)?;
                if manifest.kind != ObjectKind::TestDefinition.as_str() {
                    return Err(Error::Integrity(
                        "reviewed test manifest has wrong kind".into(),
                    ));
                }
                let test = self.load::<TestDefinition>(&manifest.object)?;
                let mut expected: BTreeSet<_> = test.inputs.into_iter().collect();
                expected.insert(test.procedure);
                expected.insert(definition.clone());
                if record.inputs.keys().cloned().collect::<BTreeSet<_>>() != expected
                    || !record.requirements.is_empty()
                {
                    return Err(Error::Integrity(
                        "test evidence omitted declared inputs".into(),
                    ));
                }
            }
        }
        if let Some(snapshot) = &record.override_snapshot {
            let graph = self.checked_graph(snapshot)?;
            let (inputs, requirements) = self.review_inputs(&graph, &record.subject)?;
            if record.inputs != inputs || record.requirements != requirements {
                return Err(Error::Integrity(
                    "override does not match its snapshot".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn record_covers_snapshot(
        &self,
        record: &ValidationRecord,
        snapshot: &ObjectId,
    ) -> Result<bool> {
        let graph = self.graph_snapshot(snapshot)?;
        match self.review_inputs(&graph, &record.subject) {
            Ok((inputs, requirements)) => {
                Ok(record.inputs == inputs && record.requirements == requirements)
            }
            Err(Error::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }
    pub fn record_review(
        &self,
        subject: ReviewSubject,
        outcome: ReviewOutcome,
        reviewer: &str,
        notes: &str,
        environment: &str,
        artifacts: &[String],
    ) -> Result<ObjectId> {
        self.record_review_details(
            subject,
            ReviewDetails {
                outcome,
                reviewer: reviewer.into(),
                notes: notes.into(),
                environment: environment.into(),
                artifacts: artifacts.to_vec(),
                overridden: false,
            },
        )
    }
    pub fn record_review_details(
        &self,
        subject: ReviewSubject,
        details: ReviewDetails,
    ) -> Result<ObjectId> {
        let _reads = self.read_scope();
        let ReviewDetails {
            outcome,
            reviewer,
            notes,
            environment,
            artifacts,
            overridden,
        } = details;
        self.ensure_ready()?;
        let graph = self.graph(None)?;
        let (inputs, requirements) = self.review_inputs(&graph, &subject)?;
        if outcome == ReviewOutcome::Passed
            && matches!(subject, ReviewSubject::Dependency { .. })
            && graph
                .edges
                .iter()
                .any(|e| requirements.contains(&e.id) && e.status != RequirementStatus::Satisfied)
        {
            return Err(Error::Conflict("update the declared requirement to the reviewed revisions before recording a passed review".into()));
        }
        for (asset, revision) in &inputs {
            let node = graph
                .nodes
                .iter()
                .find(|n| n.asset == *asset)
                .ok_or_else(|| Error::Integrity("review input missing".into()))?;
            let expected = self.load::<Revision>(revision)?;
            if self.backend.file_state(&node.path, expected.executable)?
                != Some(FileState {
                    blob: expected.blob,
                    executable: expected.executable,
                })
            {
                return Err(Error::Conflict(format!(
                    "stage review input {} before recording evidence",
                    node.path
                )));
            }
        }
        let name = review_ref(&subject)?;
        let parent = self
            .backend
            .read_meta(&name)?
            .map(|s| parse_pointer(&s))
            .transpose()?;
        let mut blobs = Vec::new();
        for artifact in &artifacts {
            blobs.push(
                self.backend
                    .store_file(artifact, false)?
                    .ok_or_else(|| Error::NotFound(artifact.clone()))?
                    .blob,
            );
        }
        let now = timestamp()?;
        let record = ValidationRecord {
            subject,
            parent,
            inputs,
            requirements,
            outcome,
            reviewer,
            started: now.clone(),
            finished: now,
            environment,
            notes,
            artifacts: blobs,
            override_snapshot: if overridden {
                Some(graph.snapshot)
            } else {
                None
            },
        };
        self.verify_validation_record(&record)?;
        let id = self.put(&record)?;
        let mut tx = Transaction::new("record manual verification");
        tx.metadata
            .push(self.meta_change(&name, Some(pointer(&id)))?);
        self.execute(tx)?;
        Ok(id)
    }
    pub fn review_history(
        &self,
        subject: &ReviewSubject,
    ) -> Result<Vec<(ObjectId, ValidationRecord)>> {
        let _reads = self.read_scope();
        let mut next = self
            .backend
            .read_meta(&review_ref(subject)?)?
            .map(|s| parse_pointer(&s))
            .transpose()?;
        let mut history = Vec::new();
        let mut seen = BTreeSet::new();
        while let Some(id) = next {
            if !seen.insert(id.clone()) {
                return Err(Error::Integrity("review history cycle".into()));
            }
            let record = self.load::<ValidationRecord>(&id)?;
            self.verify_validation_record(&record)?;
            if record.subject != *subject {
                return Err(Error::Integrity("review ref subject mismatch".into()));
            }
            next = record.parent.clone();
            history.push((id, record));
        }
        Ok(history)
    }
    fn review_entry(
        &self,
        graph: &GraphReport,
        subject: ReviewSubject,
        path: String,
        target_path: Option<String>,
    ) -> Result<VerificationEntry> {
        let history = self.review_history(&subject)?;
        let inputs = self.review_inputs(graph, &subject);
        if let Ok((inputs, requirements)) = inputs {
            for (id, record) in &history {
                if record.inputs == inputs && record.requirements == requirements {
                    return Ok(VerificationEntry {
                        subject,
                        path,
                        target_path,
                        state: match record.outcome {
                            ReviewOutcome::Passed => VerificationState::Verified,
                            ReviewOutcome::Failed => VerificationState::Failed,
                            ReviewOutcome::Error => VerificationState::Error,
                        },
                        record: Some(id.clone()),
                        reason: record.notes.clone(),
                        overridden: record.override_snapshot.is_some(),
                    });
                }
            }
        }
        Ok(VerificationEntry {
            subject,
            path,
            target_path,
            state: if history.is_empty() {
                VerificationState::Unverified
            } else {
                VerificationState::Stale
            },
            record: history.first().map(|(id, _)| id.clone()),
            reason: if history.is_empty() {
                "No verification recorded"
            } else {
                "Recorded inputs do not match the selected revisions"
            }
            .into(),
            overridden: false,
        })
    }
    pub fn verification_snapshot(&self, graph: &GraphReport) -> Result<VerificationReport> {
        let _reads = self.read_scope();
        let mut dependencies = Vec::new();
        let mut pairs = BTreeSet::new();
        for edge in &graph.edges {
            if pairs.insert((edge.dependent.clone(), edge.target.clone())) {
                dependencies.push(self.review_entry(
                    graph,
                    ReviewSubject::Dependency {
                        dependent: edge.dependent.clone(),
                        target: edge.target.clone(),
                    },
                    edge.dependent_path.clone(),
                    edge.target_path.clone(),
                )?);
            }
        }
        let mut tests = Vec::new();
        for test in self.tests_in_snapshot(&graph.snapshot)? {
            tests.push(self.review_entry(
                graph,
                ReviewSubject::Test {
                    definition: test.asset,
                },
                test.definition.name,
                None,
            )?);
        }
        let mut assets = Vec::new();
        for node in &graph.nodes {
            let subject = ReviewSubject::Asset {
                asset: node.asset.clone(),
            };
            if self.backend.read_meta(&review_ref(&subject)?)?.is_some() {
                assets.push(self.review_entry(graph, subject, node.path.clone(), None)?);
            }
        }
        let mut upstream = Vec::new();
        for entry in &dependencies {
            if entry.state == VerificationState::Verified {
                continue;
            }
            if let ReviewSubject::Dependency { dependent, target } = &entry.subject {
                let mut queue = VecDeque::from([dependent.clone()]);
                let mut seen = BTreeSet::from([dependent.clone()]);
                while let Some(asset) = queue.pop_front() {
                    for edge in graph.edges.iter().filter(|e| e.target == asset) {
                        if seen.insert(edge.dependent.clone()) {
                            upstream.push(UpstreamWarning {
                                asset: edge.dependent.clone(),
                                path: edge.dependent_path.clone(),
                                via: asset.clone(),
                                dependent: dependent.clone(),
                                target: target.clone(),
                            });
                            queue.push_back(edge.dependent.clone());
                        }
                    }
                }
            }
        }
        Ok(VerificationReport {
            snapshot: graph.snapshot.clone(),
            dependencies,
            tests,
            assets,
            upstream,
        })
    }
    pub fn verification(&self, at: Option<&str>) -> Result<VerificationReport> {
        let _reads = self.read_scope();
        self.verification_snapshot(&self.graph(at)?)
    }

    /// Explicit operator override: rebind accepted requirements, then attest the final inputs.
    pub fn override_dependency_reviews(
        &self,
        subjects: &[ReviewSubject],
        reviewer: &str,
        notes: &str,
    ) -> Result<Vec<ObjectId>> {
        let _reads = self.read_scope();
        self.ensure_ready()?;
        if reviewer.trim().is_empty() || notes.trim().is_empty() {
            return Err(Error::Invalid(
                "override requires reviewer and notes".into(),
            ));
        }
        let graph = self.graph(None)?;
        let mut accepted = BTreeSet::new();
        for subject in subjects {
            if let ReviewSubject::Dependency { dependent, target } = subject {
                accepted.insert((dependent.clone(), target.clone()));
            } else {
                return Err(Error::Invalid(
                    "override command accepts dependency reviews only".into(),
                ));
            }
        }
        let mut owners: BTreeSet<_> = accepted.iter().map(|(owner, _)| owner.clone()).collect();
        let assets: BTreeSet<_> = accepted
            .iter()
            .flat_map(|(owner, target)| [owner, target])
            .collect();
        for asset in assets {
            let node = graph
                .nodes
                .iter()
                .find(|node| node.asset == *asset)
                .ok_or_else(|| Error::Conflict("override target is absent".into()))?;
            if self.backend.file_state(&node.path, node.executable)?
                != Some(FileState {
                    blob: node.blob.clone(),
                    executable: node.executable,
                })
            {
                return Err(Error::Conflict(format!(
                    "stage {} before overriding its review",
                    node.path
                )));
            }
        }
        for (owner, target) in &accepted {
            let edges: Vec<_> = graph
                .edges
                .iter()
                .filter(|e| e.dependent == *owner && e.target == *target)
                .collect();
            if edges.is_empty() {
                return Err(Error::NotFound("dependency to override".into()));
            }
            for edge in edges {
                if edge.selected_revision.is_none()
                    || matches!(edge.requirement, DependencyRequirement::Interface { .. })
                        && edge.selected_interface.is_none()
                {
                    return Err(Error::Conflict("cannot override a missing asset or interface; restore or remove the relationship explicitly".into()));
                }
            }
        }
        let mut order = Vec::new();
        while !owners.is_empty() {
            let ready=owners.iter().find(|owner| !graph.edges.iter().any(|e| e.dependent==**owner && owners.contains(&e.target) && !matches!(e.requirement,DependencyRequirement::Interface {..}))).cloned().ok_or_else(||Error::Conflict("exact-revision dependency cycle cannot be rebound; use explicit interface contracts".into()))?;
            owners.remove(&ready);
            order.push(ready);
        }
        if accepted.is_empty() {
            return Ok(Vec::new());
        }
        let mut tree = self.tree(&graph.snapshot)?;
        let mut selected: BTreeMap<_, _> = graph
            .nodes
            .iter()
            .map(|n| (n.asset.clone(), n.revision.clone()))
            .collect();
        let mut baseline = BTreeMap::new();
        for id in self.head_tree()?.values() {
            let revision = self.load::<Revision>(id)?;
            baseline.insert(revision.asset.clone(), (id.clone(), revision));
        }
        for owner in order {
            let node = graph
                .nodes
                .iter()
                .find(|n| n.asset == owner)
                .ok_or_else(|| Error::Integrity("override owner absent".into()))?;
            let mut revision = self.load::<Revision>(&selected[&owner])?;
            for edge in graph.edges.iter().filter(|e| {
                e.dependent == owner && accepted.contains(&(e.dependent.clone(), e.target.clone()))
            }) {
                let requirement = match &edge.requirement {
                    DependencyRequirement::Interface { name, .. } => {
                        DependencyRequirement::Interface {
                            name: name.clone(),
                            interface: edge.selected_interface.clone().ok_or_else(|| {
                                Error::Conflict("missing override interface".into())
                            })?,
                        }
                    }
                    _ => DependencyRequirement::Revision {
                        revision: selected[&edge.target].clone(),
                    },
                };
                let dependency = Dependency {
                    dependent: owner.clone(),
                    target: edge.target.clone(),
                    requirement,
                };
                let mut retained = Vec::new();
                for id in &revision.dependencies {
                    let old = self.load::<Dependency>(id)?;
                    if old.target != dependency.target
                        || old.requirement.name() != dependency.requirement.name()
                    {
                        retained.push(id.clone());
                    }
                }
                retained.push(self.put(&dependency)?);
                revision.dependencies = retained;
            }
            let id = self.save_revision(revision, baseline.remove(&owner))?;
            tree.insert(node.path.clone(), id.clone());
            selected.insert(owner, id);
        }
        let snapshot = self.save_tree(&tree)?;
        let final_graph = self.graph_snapshot(&snapshot)?;
        let mut tx = Transaction::new("override dependency reviews");
        if snapshot != graph.snapshot {
            tx.metadata
                .push(self.meta_change("index", Some(pointer(&snapshot)))?);
        }
        let mut records = Vec::new();
        let mut updates = BTreeMap::new();
        for subject in subjects {
            let name = review_ref(subject)?;
            let change = updates
                .entry(name.clone())
                .or_insert(self.meta_change(&name, None)?);
            let parent = change
                .after
                .as_ref()
                .or(change.before.as_ref())
                .map(|s| parse_pointer(s))
                .transpose()?;
            let (inputs, requirements) = self.review_inputs(&final_graph, subject)?;
            let now = timestamp()?;
            let record = ValidationRecord {
                subject: subject.clone(),
                parent,
                inputs,
                requirements,
                outcome: ReviewOutcome::Passed,
                reviewer: reviewer.into(),
                notes: notes.into(),
                environment: "operator override; no engineering test claimed".into(),
                artifacts: vec![],
                started: now.clone(),
                finished: now,
                override_snapshot: Some(snapshot.clone()),
            };
            self.verify_validation_record(&record)?;
            let id = self.put(&record)?;
            change.after = Some(pointer(&id));
            records.push(id);
        }
        tx.metadata.extend(updates.into_values());
        self.execute(tx)?;
        Ok(records)
    }

    fn write_manifest<T: StoredObject>(&self, path: &str, value: &T) -> Result<ObjectId> {
        let id = self.put(value)?;
        let bytes = canonical(&Manifest {
            kind: T::KIND.as_str().into(),
            object: id.clone(),
        })?;
        self.write_asset(path, &bytes)?;
        Ok(id)
    }

    /// Write and stage bytes through the same guarded, recoverable core used by adapters.
    pub fn write_asset(&self, path: &str, bytes: &[u8]) -> Result<ObjectId> {
        if self.backend.is_bare() {
            return Err(Error::Invalid("bare repository has no working tree".into()));
        }
        self.ensure_ready()?;
        validate_path(path)?;
        let mut tree = self.index_tree()?;
        let old = tree
            .get(path)
            .map(|id| self.load::<Revision>(id))
            .transpose()?;
        let before = self
            .backend
            .file_state(path, old.as_ref().is_some_and(|r| r.executable))?;
        let expected = old.as_ref().map(|r| FileState {
            blob: r.blob.clone(),
            executable: r.executable,
        });
        if before != expected {
            return Err(Error::Conflict(format!(
                "manifest path {path} has unstaged or untracked contents"
            )));
        }
        let blob = self.backend.put_object(ObjectKind::Blob, bytes)?;
        if old
            .as_ref()
            .is_some_and(|r| r.blob == blob && !r.executable)
        {
            return self.index_id();
        }
        let asset = if let Some(old) = &old {
            old.asset.clone()
        } else {
            self.put(&Asset {
                identity: Uuid::new_v4(),
            })?
        };
        let mut parent = None;
        for id in self.head_tree()?.values() {
            if self.load::<Revision>(id)?.asset == asset {
                parent = Some(id.clone());
                break;
            }
        }
        let revision = Revision {
            asset,
            blob: blob.clone(),
            parents: parent.clone().into_iter().collect(),
            executable: false,
            dependencies: old
                .as_ref()
                .map(|r| r.dependencies.clone())
                .unwrap_or_default(),
            interfaces: old.map(|r| r.interfaces).unwrap_or_default(),
        };
        let revision_id = if let Some(id) = parent
            && same_revision_state(&revision, &self.load::<Revision>(&id)?)
        {
            id
        } else {
            self.put(&revision)?
        };
        tree.insert(path.into(), revision_id);
        self.check_destinations(&self.index_tree()?, &tree)?;
        let snapshot = self.save_tree(&tree)?;
        let mut tx = Transaction::new("write versioned engineering manifest");
        tx.files.push(FileChange {
            path: path.into(),
            before,
            after: Some(FileState {
                blob,
                executable: false,
            }),
        });
        tx.metadata
            .push(self.meta_change("index", Some(pointer(&snapshot)))?);
        self.execute(tx)?;
        Ok(snapshot)
    }
    pub fn define_test(
        &self,
        name: &str,
        procedure: &str,
        inputs: &[String],
        description: &str,
    ) -> Result<ObjectId> {
        validate_ref_name(name)?;
        let mut assets = inputs
            .iter()
            .map(|path| self.asset_at(path))
            .collect::<Result<Vec<_>>>()?;
        assets.sort();
        assets.dedup();
        let definition = TestDefinition {
            name: name.into(),
            procedure: self.asset_at(procedure)?,
            inputs: assets,
            description: description.into(),
        };
        self.write_manifest(&format!("tests/{name}.knot-test.json"), &definition)
    }
    pub fn test_subject(&self, name: &str) -> Result<ReviewSubject> {
        let test = self
            .tests_in_snapshot(&self.index_id()?)?
            .into_iter()
            .find(|t| t.definition.name == name || t.asset.as_str() == name)
            .ok_or_else(|| Error::NotFound(format!("test {name}")))?;
        Ok(ReviewSubject::Test {
            definition: test.asset,
        })
    }
    pub fn add_trace(&self, source: &str, target: &str, relation: &str) -> Result<ObjectId> {
        let graph = self.graph(None)?;
        let source = self.node_for(&graph, source)?;
        let target = self.node_for(&graph, target)?;
        let link = TraceLink {
            source: source.asset.clone(),
            source_revision: source.revision.clone(),
            target: target.asset.clone(),
            target_revision: target.revision.clone(),
            relation: relation.into(),
        };
        link.validate()?;
        let key = object_id(
            ObjectKind::TraceLink,
            &canonical(&(&link.source, &link.target, &link.relation))?,
        );
        self.write_manifest(&format!("docs/trace/{key}.knot-link.json"), &link)
    }
    pub fn traces(
        &self,
        asset: Option<&str>,
        at: Option<&str>,
        reverse: bool,
        transitive: bool,
    ) -> Result<serde_json::Value> {
        let graph = self.graph(at)?;
        let mut links = Vec::new();
        for node in &graph.nodes {
            if let Some(id) = self.manifest_id(node, ObjectKind::TraceLink, ".knot-link.json")? {
                let link = self.load::<TraceLink>(&id)?;
                let current = graph
                    .nodes
                    .iter()
                    .any(|n| n.asset == link.source && n.revision == link.source_revision)
                    && graph
                        .nodes
                        .iter()
                        .any(|n| n.asset == link.target && n.revision == link.target_revision);
                links.push((id, link, current));
            }
        }
        if let Some(asset) = asset {
            let root = self.node_for(&graph, asset)?.asset.clone();
            let mut seen = BTreeSet::from([root]);
            let mut chosen = BTreeSet::new();
            loop {
                let mut next = Vec::new();
                for (index, (_, link, _)) in links.iter().enumerate() {
                    let (from, to) = if reverse {
                        (&link.target, &link.source)
                    } else {
                        (&link.source, &link.target)
                    };
                    if seen.contains(from) {
                        chosen.insert(index);
                        next.push(to.clone());
                    }
                }
                let old = seen.len();
                seen.extend(next);
                if !transitive || old == seen.len() {
                    break;
                }
            }
            links = links
                .into_iter()
                .enumerate()
                .filter(|(i, _)| chosen.contains(i))
                .map(|(_, link)| link)
                .collect();
        }
        Ok(
            serde_json::json!({"snapshot":graph.snapshot,"links":links.into_iter().map(|(id,link,current)|serde_json::json!({"id":id,"link":link,"current":current})).collect::<Vec<_>>()}),
        )
    }
    pub fn release_policy(&self, snapshot: &ObjectId) -> Result<ReleasePolicy> {
        if let Some(revision) = self.tree(snapshot)?.get("knot-policy.json") {
            let revision = self.load::<Revision>(revision)?;
            let (_, bytes) = self.backend.get_object(&revision.blob)?;
            Ok(serde_json::from_slice(&bytes)?)
        } else {
            Ok(ReleasePolicy::default())
        }
    }
    pub fn require_validated_snapshot(&self, snapshot: &ObjectId) -> Result<Vec<ObjectId>> {
        self.require_satisfied(snapshot)?;
        let policy = self.release_policy(snapshot)?;
        let graph = self.graph_snapshot(snapshot)?;
        let report = self.verification_snapshot(&graph)?;
        let mut evidence = BTreeSet::new();
        let mut required = Vec::new();
        if policy.require_dependency_reviews {
            required.extend(report.dependencies.iter());
        }
        if policy.require_all_tests {
            required.extend(report.tests.iter());
        }
        for name in &policy.required_tests {
            required.push(
                report
                    .tests
                    .iter()
                    .find(|t| t.path == *name)
                    .ok_or_else(|| Error::Conflict(format!("required test {name} is absent")))?,
            );
        }
        for asset in &policy.required_assets {
            let node = graph
                .nodes
                .iter()
                .find(|n| n.asset == *asset)
                .ok_or_else(|| Error::Conflict("required review asset absent".into()))?;
            let entry = self.review_entry(
                &graph,
                ReviewSubject::Asset {
                    asset: asset.clone(),
                },
                node.path.clone(),
                None,
            )?;
            if entry.state != VerificationState::Verified {
                return Err(Error::Conflict(format!(
                    "asset {} requires verification",
                    node.path
                )));
            }
            evidence.extend(entry.record);
        }
        for entry in required {
            if entry.state != VerificationState::Verified {
                return Err(Error::Conflict(format!(
                    "{} needs verification ({:?})",
                    entry.path, entry.state
                )));
            }
            evidence.extend(entry.record.clone());
        }
        Ok(evidence.into_iter().collect())
    }
    pub fn create_release(&self, name: &str, approver: &str, notes: &str) -> Result<ObjectId> {
        self.ensure_ready()?;
        validate_ref_name(name)?;
        let status = self.status()?;
        if !status.staged.is_empty() || !status.unstaged.is_empty() {
            return Err(Error::Conflict(
                "release requires a committed, clean tracked state".into(),
            ));
        }
        let commit = self
            .head_id()?
            .ok_or_else(|| Error::NotFound("HEAD".into()))?;
        let snapshot = self.load::<Commit>(&commit)?.snapshot;
        let release = Release {
            name: name.into(),
            commit,
            snapshot: snapshot.clone(),
            policy: self.release_policy(&snapshot)?,
            evidence: self.require_validated_snapshot(&snapshot)?,
            approver: approver.into(),
            timestamp: timestamp()?,
            notes: notes.into(),
        };
        let reference = format!("refs/releases/{name}");
        if self.backend.read_meta(&reference)?.is_some() {
            return Err(Error::Conflict("release already exists".into()));
        }
        self.check_reference_name(&reference)?;
        let id = self.put(&release)?;
        let mut tx = Transaction::new("create approved release");
        tx.metadata
            .push(self.meta_change(&reference, Some(pointer(&id)))?);
        self.execute(tx)?;
        Ok(id)
    }
    pub fn verify_release(&self, release: &Release) -> Result<()> {
        release.validate()?;
        if self.load::<Commit>(&release.commit)?.snapshot != release.snapshot
            || self.release_policy(&release.snapshot)? != release.policy
        {
            return Err(Error::Integrity(
                "release snapshot or policy mismatch".into(),
            ));
        }
        self.require_satisfied(&release.snapshot)?;
        let graph = self.graph_snapshot(&release.snapshot)?;
        let mut subjects = Vec::new();
        if release.policy.require_dependency_reviews {
            for edge in &graph.edges {
                subjects.push(ReviewSubject::Dependency {
                    dependent: edge.dependent.clone(),
                    target: edge.target.clone(),
                });
            }
        }
        let tests = self.tests_in_snapshot(&release.snapshot)?;
        for test in &tests {
            if release.policy.require_all_tests
                || release
                    .policy
                    .required_tests
                    .contains(&test.definition.name)
            {
                subjects.push(ReviewSubject::Test {
                    definition: test.asset.clone(),
                });
            }
        }
        if release
            .policy
            .required_tests
            .iter()
            .any(|name| !tests.iter().any(|test| test.definition.name == *name))
        {
            return Err(Error::Integrity("release required test missing".into()));
        }
        subjects.extend(
            release
                .policy
                .required_assets
                .iter()
                .map(|asset| ReviewSubject::Asset {
                    asset: asset.clone(),
                }),
        );
        let mut records = Vec::new();
        for id in &release.evidence {
            let record = self.load::<ValidationRecord>(id)?;
            self.verify_validation_record(&record)?;
            records.push(record);
        }
        for subject in subjects {
            let (inputs, requirements) = self.review_inputs(&graph, &subject)?;
            if !records.iter().any(|record| {
                record.subject == subject
                    && record.inputs == inputs
                    && record.requirements == requirements
                    && record.outcome == ReviewOutcome::Passed
            }) {
                return Err(Error::Integrity(
                    "release lacks applicable passed evidence".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn releases(&self) -> Result<Vec<(ObjectId, Release)>> {
        self.backend
            .list_meta("refs/releases/")?
            .into_iter()
            .map(|name| {
                let id = parse_pointer(
                    &self
                        .backend
                        .read_meta(&name)?
                        .ok_or_else(|| Error::Integrity("missing release".into()))?,
                )?;
                let release = self.load::<Release>(&id)?;
                Ok((id, release))
            })
            .collect()
    }
}
