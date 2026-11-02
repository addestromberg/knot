#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::*;
use knot_store::FsBackend;
use std::fs;
use tempfile::tempdir;

fn revision(repo: &Repository<FsBackend>, path: &str) -> ObjectId {
    repo.dependencies(path, None, false, false)
        .unwrap()
        .root
        .revision
}
fn pin(repo: &Repository<FsBackend>, owner: &str, target: &str) {
    repo.add_dependency(owner, target, DependencyRequest::Revision(None))
        .unwrap();
}

#[test]
fn resolver_selects_compatible_history_and_preview_is_read_only() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    for name in ["housing", "pcb", "firmware"] {
        fs::write(dir.path().join(name), "R1").unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    pin(&repo, "pcb", "housing");
    pin(&repo, "firmware", "pcb");
    repo.commit("First compatible assembly").unwrap();
    repo.create_ref("first", "HEAD", true).unwrap();
    let pcb1 = revision(&repo, "pcb");
    for name in ["housing", "pcb", "firmware"] {
        fs::write(dir.path().join(name), "R2").unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    pin(&repo, "pcb", "housing");
    pin(&repo, "firmware", "pcb");
    repo.commit("Second compatible assembly").unwrap();
    let housing2 = revision(&repo, "housing");
    let firmware2 = revision(&repo, "firmware");
    fs::write(dir.path().join("firmware"), "R3 supports old PCB").unwrap();
    repo.add(&["firmware".into()]).unwrap();
    repo.add_dependency(
        "firmware",
        "pcb",
        DependencyRequest::Revision(Some(pcb1.clone())),
    )
    .unwrap();
    let head = repo
        .commit_with_options("WIP incompatible latest firmware", true)
        .unwrap();
    let before = repo.backend().list_objects().unwrap();
    let pins = [ResolutionPin {
        asset: "housing".into(),
        revision: housing2,
    }];
    let plan = repo.plan_resolution(&pins).unwrap();
    assert_eq!(
        plan.snapshot
            .entries
            .iter()
            .find(|e| e.path == "firmware")
            .unwrap()
            .revision,
        firmware2
    );
    assert_eq!(repo.backend().list_objects().unwrap(), before);
    assert_eq!(
        repo.plan_resolution(&pins).unwrap().snapshot_id,
        plan.snapshot_id
    );
    assert!(!plan.validated);
    repo.apply_resolution(&plan).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("firmware")).unwrap(),
        "R2"
    );
    assert_eq!(repo.head_id().unwrap(), Some(head));
    assert!(repo.graph(None).unwrap().requirements_satisfied);
    repo.commit("Resolve compatible configuration").unwrap();
    let bad = repo
        .plan_resolution(&[
            pins[0].clone(),
            ResolutionPin {
                asset: "pcb".into(),
                revision: pcb1,
            },
        ])
        .unwrap_err();
    assert_eq!(bad.code(), "resolution_conflict");
    assert!(!bad.details()["conflicts"].as_array().unwrap().is_empty());
    repo.fsck().unwrap();
}

#[test]
fn ranges_are_inclusive_ancestry_constraints_and_dirty_checkout_is_protected() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    fs::write(dir.path().join("pipe"), "R1").unwrap();
    fs::write(dir.path().join("rod"), "rod").unwrap();
    repo.add(&[".".into()]).unwrap();
    repo.commit("One").unwrap();
    let first = revision(&repo, "pipe");
    fs::write(dir.path().join("pipe"), "R2").unwrap();
    repo.add(&["pipe".into()]).unwrap();
    repo.commit("Two").unwrap();
    let second = revision(&repo, "pipe");
    repo.add_dependency(
        "rod",
        "pipe",
        DependencyRequest::RevisionRange {
            minimum: first.clone(),
            maximum: Some(second.clone()),
        },
    )
    .unwrap();
    repo.commit("Inclusive pipe range").unwrap();
    fs::write(dir.path().join("pipe"), "R3").unwrap();
    repo.add(&["pipe".into()]).unwrap();
    assert!(!repo.graph(None).unwrap().requirements_satisfied);
    repo.commit_with_options("Outside range", true).unwrap();
    let plan = repo.plan_resolution(&[]).unwrap();
    assert_eq!(
        plan.snapshot
            .entries
            .iter()
            .find(|e| e.path == "pipe")
            .unwrap()
            .revision,
        second
    );
    fs::write(dir.path().join("pipe"), "external unsaved changes").unwrap();
    assert!(repo.apply_resolution(&plan).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("pipe")).unwrap(),
        "external unsaved changes"
    );
    repo.restore(&["pipe".into()], None, false, false).unwrap();
    repo.apply_resolution(&plan).unwrap();
    repo.commit("Restore newest compatible pipe").unwrap();
    assert!(
        repo.add_dependency(
            "rod",
            "pipe",
            DependencyRequest::RevisionRange {
                minimum: second,
                maximum: Some(first)
            }
        )
        .is_err()
    );
    assert!(
        fs::read_to_string(dir.path().join(".knot/config.toml"))
            .unwrap()
            .contains("format_version = 3")
    );
    repo.fsck().unwrap();
}
