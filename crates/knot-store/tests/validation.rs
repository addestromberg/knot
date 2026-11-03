#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::*;
use knot_store::FsBackend;
use std::fs;
use tempfile::tempdir;

fn review(repo: &Repository<FsBackend>, subject: ReviewSubject) -> ObjectId {
    repo.record_review(
        subject,
        ReviewOutcome::Passed,
        "Engineer",
        "Measured and reviewed",
        "manual bench inspection",
        &[],
    )
    .unwrap()
}

#[test]
fn release_checkout_detaches_without_rewinding_a_same_spelled_branch() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    repo.write_asset("part", b"A").unwrap();
    let first = repo.commit("Initial").unwrap();
    repo.create_release("v1", "Engineer", "Approved").unwrap();
    repo.write_asset("part", b"B").unwrap();
    let second = repo.commit("Newer").unwrap();
    repo.create_ref("release/v1", "HEAD", false).unwrap();
    let branches = repo.refs(false).unwrap();
    repo.apply_checkout(repo.plan_checkout("release/v1").unwrap())
        .unwrap();
    assert_eq!(repo.status().unwrap().branch, None);
    assert_eq!(repo.head_id().unwrap(), Some(first));
    assert_eq!(repo.refs(false).unwrap(), branches);
    assert_eq!(repo.refs(false).unwrap()["release/v1"], second);
    assert_eq!(fs::read(dir.path().join("part")).unwrap(), b"A");
    repo.fsck().unwrap();
}

#[test]
fn write_reverts_restore_revision_ids_and_exact_input_evidence() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    repo.write_asset("part", b"A").unwrap();
    repo.write_asset("procedure.md", b"Measure part").unwrap();
    repo.define_test("measure", "procedure.md", &["part".into()], "Original")
        .unwrap();
    repo.commit("Tested part").unwrap();
    let record = review(&repo, repo.test_subject("measure").unwrap());
    let original = repo.status().unwrap().staged_snapshot;
    let revision = repo
        .dependencies("part", None, false, false)
        .unwrap()
        .root
        .revision;
    repo.write_asset("part", b"B").unwrap();
    let changed = repo
        .dependencies("part", None, false, false)
        .unwrap()
        .root
        .revision;
    assert_eq!(
        repo.load::<Revision>(&changed).unwrap().parents,
        vec![revision.clone()]
    );
    assert_eq!(
        repo.verification(None).unwrap().tests[0].state,
        VerificationState::Stale
    );
    assert_eq!(repo.write_asset("part", b"A").unwrap(), original);
    assert_eq!(
        repo.verification(None).unwrap().tests[0].record,
        Some(record.clone())
    );
    assert_eq!(
        repo.verification(None).unwrap().tests[0].state,
        VerificationState::Verified
    );
    repo.define_test("measure", "procedure.md", &["part".into()], "Changed")
        .unwrap();
    assert_ne!(repo.status().unwrap().staged_snapshot, original);
    repo.define_test("measure", "procedure.md", &["part".into()], "Original")
        .unwrap();
    assert_eq!(repo.status().unwrap().staged_snapshot, original);
    assert_eq!(
        repo.verification(None).unwrap().tests[0].record,
        Some(record)
    );
    assert!(repo.status().unwrap().staged.is_empty());
    assert!(repo.diff(true).unwrap().is_empty());
    repo.fsck().unwrap();
}

#[test]
fn unchanged_contract_does_not_hide_stale_manual_evidence_and_history_is_reusable() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    for (name, contents) in [
        ("pipe", "pipe R1"),
        ("rod", "rod"),
        ("assembly", "assembly"),
        ("fit.txt", "bore20"),
    ] {
        fs::write(root.join(name), contents).unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    repo.set_interface("pipe", "bore", "fit.txt").unwrap();
    repo.add_dependency("rod", "pipe", DependencyRequest::Interface("bore".into()))
        .unwrap();
    repo.add_dependency("assembly", "rod", DependencyRequest::Revision(None))
        .unwrap();
    repo.commit("Assembly").unwrap();
    repo.create_ref("original", "HEAD", true).unwrap();
    let subject = repo.review_subject("rod", Some("pipe")).unwrap();
    let first = review(&repo, subject.clone());
    review(&repo, repo.review_subject("assembly", Some("rod")).unwrap());
    let before = repo.status().unwrap();
    assert!(
        before
            .verification
            .dependencies
            .iter()
            .all(|e| e.state == VerificationState::Verified)
    );
    fs::write(root.join("pipe"), "pipe R2 external mount").unwrap();
    repo.add(&["pipe".into()]).unwrap();
    let status = repo.status().unwrap();
    assert!(status.requirements_satisfied);
    let entry = status
        .verification
        .dependencies
        .iter()
        .find(|e| e.path == "rod")
        .unwrap();
    assert_eq!(entry.state, VerificationState::Stale);
    assert_eq!(status.verification.upstream[0].path, "assembly");
    assert!(
        repo.require_validated_snapshot(&status.staged_snapshot)
            .is_err()
    );
    repo.commit("External mount").unwrap();
    review(&repo, subject.clone());
    assert!(repo.verification(None).unwrap().upstream.is_empty());
    let failed = repo
        .record_review(
            subject.clone(),
            ReviewOutcome::Failed,
            "Engineer",
            "Measured interference",
            "bench",
            &[],
        )
        .unwrap();
    assert_ne!(failed, first);
    assert_eq!(
        repo.verification(None)
            .unwrap()
            .dependencies
            .iter()
            .find(|e| e.path == "rod")
            .unwrap()
            .state,
        VerificationState::Failed
    );
    repo.apply_checkout(repo.plan_checkout("original").unwrap())
        .unwrap();
    let report = repo.verification(None).unwrap();
    assert_eq!(
        report
            .dependencies
            .iter()
            .find(|e| e.path == "rod")
            .unwrap()
            .record,
        Some(first)
    );
    assert_eq!(repo.review_history(&subject).unwrap().len(), 3);
    repo.fsck().unwrap();
}

#[test]
fn versioned_tests_traceability_and_release_gates_follow_exact_inputs() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    for (name, contents) in [
        ("pipe", "pipe"),
        ("rod", "rod"),
        ("fit.md", "Measure rod clearance"),
        ("requirement.md", "REQ-42: sliding fit"),
    ] {
        fs::write(root.join(name), contents).unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    repo.add_dependency("rod", "pipe", DependencyRequest::Revision(None))
        .unwrap();
    repo.define_test(
        "fit",
        "fit.md",
        &["pipe".into(), "rod".into()],
        "Manual sliding fit",
    )
    .unwrap();
    repo.add_trace("rod", "requirement.md", "satisfies")
        .unwrap();
    repo.add_trace("fit.md", "requirement.md", "verifies")
        .unwrap();
    repo.commit("Define traceable fit test").unwrap();
    repo.create_ref("baseline", "HEAD", true).unwrap();
    assert_eq!(
        repo.tests_in_snapshot(&repo.status().unwrap().staged_snapshot)
            .unwrap()
            .len(),
        1
    );
    assert!(repo.create_release("v1", "Approver", "Release").is_err());
    review(&repo, repo.review_subject("rod", Some("pipe")).unwrap());
    review(&repo, repo.test_subject("fit").unwrap());
    let head = repo.head_id().unwrap();
    let release = repo
        .create_release("v1", "Approver", "Measured clearance")
        .unwrap();
    assert_eq!(repo.head_id().unwrap(), head);
    assert_eq!(repo.load::<Release>(&release).unwrap().evidence.len(), 2);
    assert!(repo.create_release("v1", "Approver", "Again").is_err());
    assert_eq!(repo.resolve("release/v1").unwrap(), head.unwrap());
    assert!(
        repo.traces(Some("requirement.md"), None, true, true)
            .unwrap()["links"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["current"] == true)
    );
    fs::write(root.join("fit.md"), "New measurement procedure").unwrap();
    repo.add(&["fit.md".into()]).unwrap();
    assert_eq!(
        repo.verification(None).unwrap().tests[0].state,
        VerificationState::Stale
    );
    assert!(
        repo.traces(None, None, false, false).unwrap()["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["current"] == false)
    );
    repo.commit("Update procedure").unwrap();
    repo.apply_checkout(repo.plan_checkout("baseline").unwrap())
        .unwrap();
    assert_eq!(
        repo.verification(None).unwrap().tests[0].state,
        VerificationState::Verified
    );
    repo.fsck().unwrap();
}

#[test]
fn overrides_rebind_dependencies_and_are_attached_to_the_commit() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    for name in ["pipe", "rod", "assembly"] {
        fs::write(root.join(name), name).unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    repo.add_dependency("rod", "pipe", DependencyRequest::Revision(None))
        .unwrap();
    repo.add_dependency("assembly", "rod", DependencyRequest::Revision(None))
        .unwrap();
    repo.commit("Initial").unwrap();
    fs::write(root.join("pipe"), "pipe changed").unwrap();
    repo.add(&["pipe".into()]).unwrap();
    let subjects = repo
        .verification(None)
        .unwrap()
        .dependencies
        .into_iter()
        .map(|e| e.subject)
        .collect::<Vec<_>>();
    let ids = repo
        .override_dependency_reviews(
            &subjects,
            "Operator",
            "Accept change without engineering verification",
        )
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert!(repo.graph(None).unwrap().requirements_satisfied);
    assert!(
        repo.verification(None)
            .unwrap()
            .dependencies
            .iter()
            .all(|e| e.state == VerificationState::Verified && e.overridden)
    );
    let commit = repo.commit("Overridden change").unwrap();
    let commit = repo.load::<Commit>(&commit).unwrap();
    assert_eq!(commit.overrides.len(), 2);
    for id in &commit.overrides {
        assert_eq!(
            repo.load::<ValidationRecord>(id).unwrap().override_snapshot,
            Some(commit.snapshot.clone())
        );
    }
    repo.fsck().unwrap();
}

#[test]
fn evidence_refuses_unstaged_inputs_and_untracked_manifest_collisions() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    fs::write(root.join("pipe"), "pipe").unwrap();
    repo.add(&[".".into()]).unwrap();
    repo.commit("Initial").unwrap();
    let subject = repo.review_subject("pipe", None).unwrap();
    fs::write(root.join("pipe"), "unstaged").unwrap();
    assert!(
        repo.record_review(subject, ReviewOutcome::Passed, "E", "checked", "bench", &[])
            .is_err()
    );
    fs::create_dir(root.join("tests")).unwrap();
    fs::write(root.join("tests/fit.knot-test.json"), "local work").unwrap();
    assert!(
        repo.define_test("fit", "pipe", &["pipe".into()], "")
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.join("tests/fit.knot-test.json")).unwrap(),
        "local work"
    );
}

#[test]
fn malformed_manifests_do_not_poison_history_and_incomplete_evidence_is_rejected() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    fs::write(root.join("asset"), "part").unwrap();
    fs::write(root.join("procedure.md"), "Inspect part").unwrap();
    repo.add(&[".".into()]).unwrap();
    repo.commit("Initial").unwrap();
    fs::create_dir(root.join("tests")).unwrap();
    fs::write(root.join("tests/bad.knot-test.json"), "broken manifest").unwrap();
    let before = repo.status().unwrap().staged_snapshot;
    assert!(repo.add(&["tests/bad.knot-test.json".into()]).is_err());
    assert_eq!(repo.status().unwrap().staged_snapshot, before);
    repo.fsck().unwrap();
    fs::remove_file(root.join("tests/bad.knot-test.json")).unwrap();
    repo.define_test("inspection", "procedure.md", &["asset".into()], "Inspect")
        .unwrap();
    repo.commit("Define inspection").unwrap();
    let id = review(&repo, repo.test_subject("inspection").unwrap());
    let mut record = repo.load::<ValidationRecord>(&id).unwrap();
    record
        .inputs
        .remove(&repo.asset_at("procedure.md").unwrap());
    assert!(repo.verify_validation_record(&record).is_err());
    let release = repo.create_release("v1", "Approver", "").unwrap();
    let mut release = repo.load::<Release>(&release).unwrap();
    release.evidence.clear();
    assert!(repo.verify_release(&release).is_err());
}

#[test]
fn failed_bulk_override_preserves_staging_and_all_review_refs() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    for name in ["pipe", "rod", "assembly"] {
        fs::write(dir.path().join(name), name).unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    repo.add_dependency("rod", "pipe", DependencyRequest::Revision(None))
        .unwrap();
    repo.add_dependency("assembly", "rod", DependencyRequest::Revision(None))
        .unwrap();
    repo.commit("Initial").unwrap();
    let subjects: Vec<_> = repo
        .verification(None)
        .unwrap()
        .dependencies
        .into_iter()
        .map(|e| e.subject)
        .collect();
    repo.override_dependency_reviews(&subjects, "Operator", "First override")
        .unwrap();
    let before = repo.backend().read_meta("index").unwrap();
    let refs: Vec<_> = subjects
        .iter()
        .map(|s| {
            let name = review_ref(s).unwrap();
            let value = repo.backend().read_meta(&name).unwrap();
            (name, value)
        })
        .collect();
    fs::write(dir.path().join("pipe"), "unstaged").unwrap();
    assert!(matches!(
        repo.override_dependency_reviews(&subjects, "Operator", "Must fail"),
        Err(Error::Conflict(_))
    ));
    assert_eq!(repo.backend().read_meta("index").unwrap(), before);
    for (name, value) in refs {
        assert_eq!(repo.backend().read_meta(&name).unwrap(), value);
    }
    assert!(
        repo.backend()
            .read_meta("transaction.json")
            .unwrap()
            .is_none()
    );
}

#[test]
fn bulk_override_rebinds_mixed_contracts_against_final_revisions() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    for name in ["pipe", "rod", "assembly", "contract"] {
        fs::write(dir.path().join(name), name).unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    repo.set_interface("rod", "fit", "contract").unwrap();
    repo.add_dependency("rod", "pipe", DependencyRequest::Revision(None))
        .unwrap();
    repo.add_dependency(
        "assembly",
        "rod",
        DependencyRequest::Interface("fit".into()),
    )
    .unwrap();
    let pipe = repo
        .graph(None)
        .unwrap()
        .nodes
        .into_iter()
        .find(|n| n.path == "pipe")
        .unwrap()
        .revision;
    repo.add_dependency(
        "assembly",
        "pipe",
        DependencyRequest::RevisionRange {
            minimum: pipe,
            maximum: None,
        },
    )
    .unwrap();
    repo.commit("Initial contracts").unwrap();
    fs::write(dir.path().join("pipe"), "new pipe").unwrap();
    fs::write(dir.path().join("contract"), "new interface").unwrap();
    repo.add(&["pipe".into(), "contract".into()]).unwrap();
    repo.set_interface("rod", "fit", "contract").unwrap();
    let subjects: Vec<_> = repo
        .verification(None)
        .unwrap()
        .dependencies
        .into_iter()
        .map(|e| e.subject)
        .collect();
    let records = repo
        .override_dependency_reviews(&subjects, "Operator", "Accept selected inputs")
        .unwrap();
    assert_eq!(records.len(), 3);
    let graph = repo.graph(None).unwrap();
    assert!(graph.requirements_satisfied);
    assert!(
        graph
            .edges
            .iter()
            .all(|e| !matches!(e.requirement, DependencyRequirement::RevisionRange { .. }))
    );
    assert_eq!(
        graph
            .edges
            .iter()
            .filter(|e| matches!(e.requirement, DependencyRequirement::Interface { .. }))
            .count(),
        1
    );
    for id in records {
        let record = repo.load::<ValidationRecord>(&id).unwrap();
        assert_eq!(record.override_snapshot, Some(graph.snapshot.clone()));
        assert!(
            repo.record_covers_snapshot(&record, &graph.snapshot)
                .unwrap()
        );
    }
    repo.fsck().unwrap();
}
