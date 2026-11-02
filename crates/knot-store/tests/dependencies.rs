#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::*;
use knot_store::FsBackend;
use std::{fs, path::Path};
use tempfile::{TempDir, tempdir};

fn setup() -> (TempDir, Repository<FsBackend>) {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    for (path, content) in [
        ("housing.FCStd", "model R5"),
        ("controller.kicad_pcb", "PCB R3"),
        ("firmware.rs", "firmware"),
        ("mount.txt", "holes: 20 mm"),
    ] {
        fs::write(dir.path().join(path), content).unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    repo.commit("Initial assets").unwrap();
    (dir, repo)
}
fn exact(repo: &Repository<FsBackend>, source: &str, target: &str) -> MetadataUpdate {
    repo.add_dependency(source, target, DependencyRequest::Revision(None))
        .unwrap()
}
fn iface(repo: &Repository<FsBackend>) {
    repo.set_interface("housing.FCStd", "pcb_mount", "mount.txt")
        .unwrap();
    repo.add_dependency(
        "controller.kicad_pcb",
        "housing.FCStd",
        DependencyRequest::Interface("pcb_mount".into()),
    )
    .unwrap();
}
fn node(repo: &Repository<FsBackend>, path: &str) -> GraphNode {
    repo.graph(None)
        .unwrap()
        .nodes
        .into_iter()
        .find(|n| n.path == path)
        .unwrap()
}
fn paths(entries: &[ImpactEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.path.as_str()).collect()
}
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn electromechanical_exact_pins_and_test_procedure_assets_follow_snapshots() {
    let (dir, repo) = setup();
    for path in ["mechanical-test.md", "electrical-test.md"] {
        fs::write(dir.path().join(path), "Manual test procedure").unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    exact(&repo, "firmware.rs", "controller.kicad_pcb");
    exact(&repo, "mechanical-test.md", "housing.FCStd");
    exact(&repo, "electrical-test.md", "controller.kicad_pcb");
    let first = repo.commit("Compatible R5 project").unwrap();
    let s5 = repo.load::<Commit>(&first).unwrap().snapshot;
    repo.create_ref("r5", "HEAD", true).unwrap();
    let old_model = node(&repo, "housing.FCStd");
    fs::write(dir.path().join("housing.FCStd"), "model R6").unwrap();
    repo.add(&["housing.FCStd".into()]).unwrap();
    let graph = repo.graph(None).unwrap();
    assert!(!graph.requirements_satisfied);
    assert_eq!(
        graph
            .edges
            .iter()
            .filter(|e| e.status == RequirementStatus::RevisionMismatch)
            .count(),
        2
    );
    let impact = repo.impact("housing.FCStd", None, Some("HEAD")).unwrap();
    assert_eq!(
        paths(&impact.direct),
        ["controller.kicad_pcb", "mechanical-test.md"]
    );
    assert_eq!(
        paths(&impact.transitive),
        ["electrical-test.md", "firmware.rs"]
    );
    assert_eq!(impact.changes, ["content"]);
    assert!(matches!(
        repo.commit("Must resolve first"),
        Err(Error::UnsatisfiedDependencies(_))
    ));
    assert_eq!(repo.head_id().unwrap(), Some(first));
    exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    exact(&repo, "mechanical-test.md", "housing.FCStd");
    exact(&repo, "firmware.rs", "controller.kicad_pcb");
    exact(&repo, "electrical-test.md", "controller.kicad_pcb");
    repo.commit("Update declared pins for R6").unwrap();
    let s6 = repo.status().unwrap().snapshot.unwrap();
    repo.apply_checkout(repo.plan_checkout("r5").unwrap())
        .unwrap();
    assert_eq!(node(&repo, "housing.FCStd").revision, old_model.revision);
    assert_eq!(repo.status().unwrap().snapshot, Some(s5));
    assert!(repo.graph(None).unwrap().requirements_satisfied);
    repo.apply_checkout(repo.plan_checkout("main").unwrap())
        .unwrap();
    assert_eq!(repo.status().unwrap().snapshot, Some(s6.clone()));
    let destination = tempdir().unwrap();
    copy_dir(&dir.path().join(".knot"), &destination.path().join(".knot"));
    let other = Repository::open(FsBackend::open(destination.path()).unwrap()).unwrap();
    other.restore(&[".".into()], None, false, false).unwrap();
    assert_eq!(other.graph(None).unwrap().snapshot, s6);
    assert!(other.graph(None).unwrap().requirements_satisfied);
    other.fsck().unwrap();
    repo.fsck().unwrap();
}

#[test]
fn unchanged_named_contract_does_not_propagate_a_cad_byte_change() {
    let (dir, repo) = setup();
    iface(&repo);
    exact(&repo, "firmware.rs", "controller.kicad_pcb");
    repo.commit("Named interface dependency").unwrap();
    let old_interface = node(&repo, "housing.FCStd").interfaces[0].id.clone();
    fs::write(
        dir.path().join("housing.FCStd"),
        "Geometry changed away from mounting holes",
    )
    .unwrap();
    repo.add(&[".".into()]).unwrap();
    assert_eq!(node(&repo, "housing.FCStd").interfaces[0].id, old_interface);
    assert!(repo.graph(None).unwrap().requirements_satisfied);
    let impact = repo.impact("housing.FCStd", None, Some("HEAD")).unwrap();
    assert_eq!(impact.changes, ["content"]);
    assert!(impact.direct.is_empty());
    assert!(impact.transitive.is_empty());
    assert_eq!(paths(&impact.unaffected_direct), ["controller.kicad_pcb"]);
    assert_eq!(
        repo.impact("housing.FCStd", None, None)
            .unwrap()
            .transitive
            .len(),
        1
    );
    repo.commit("Same declared mounting contract").unwrap();
    fs::write(dir.path().join("mount.txt"), "holes: 25 mm").unwrap();
    repo.set_interface("housing.FCStd", "pcb_mount", "mount.txt")
        .unwrap();
    let graph = repo.graph(None).unwrap();
    assert_eq!(
        graph
            .edges
            .iter()
            .find(|e| e.dependent_path == "controller.kicad_pcb")
            .unwrap()
            .status,
        RequirementStatus::InterfaceMismatch
    );
    let impact = repo.impact("housing.FCStd", None, Some("HEAD")).unwrap();
    assert_eq!(impact.changes, ["interfaces"]);
    assert_eq!(paths(&impact.direct), ["controller.kicad_pcb"]);
    assert_eq!(paths(&impact.transitive), ["firmware.rs"]);
    assert!(matches!(
        repo.commit("Contract mismatch"),
        Err(Error::UnsatisfiedDependencies(_))
    ));
    repo.fsck().unwrap();
}

#[test]
fn metadata_only_changes_are_staged_diffed_and_kept_by_add() {
    let (dir, repo) = setup();
    let pcb_before = node(&repo, "controller.kicad_pcb");
    let update = exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    let staged = repo.diff(true).unwrap();
    let diff = staged
        .iter()
        .find(|d| d.path == "controller.kicad_pcb")
        .unwrap();
    assert_eq!(diff.before, diff.after);
    assert_eq!(diff.metadata_changes, ["constraints"]);
    let index = repo.add(&[".".into()]).unwrap();
    assert_eq!(index, update.snapshot);
    assert_ne!(
        node(&repo, "controller.kicad_pcb").revision,
        pcb_before.revision
    );
    assert_eq!(node(&repo, "controller.kicad_pcb").blob, pcb_before.blob);
    repo.commit("Dependency without file change").unwrap();
    fs::write(dir.path().join("controller.kicad_pcb"), "new PCB bytes").unwrap();
    repo.add(&["controller.kicad_pcb".into()]).unwrap();
    assert_eq!(
        repo.dependencies("controller.kicad_pcb", None, false, false)
            .unwrap()
            .edges
            .len(),
        1
    );
    repo.fsck().unwrap();
}

#[test]
fn rename_and_move_preserve_dependency_and_interface_identities() {
    let (_dir, repo) = setup();
    iface(&repo);
    repo.commit("Interfaces").unwrap();
    let before = node(&repo, "housing.FCStd");
    let pin = repo
        .dependencies("controller.kicad_pcb", None, false, false)
        .unwrap()
        .edges[0]
        .id
        .clone();
    repo.move_asset("housing.FCStd", "mechanical/enclosure.FCStd", false)
        .unwrap();
    assert_eq!(
        repo.impact(&before.asset.to_string(), None, Some("HEAD"))
            .unwrap()
            .changes,
        ["path"]
    );
    assert!(
        repo.impact(&before.asset.to_string(), None, Some("HEAD"))
            .unwrap()
            .direct
            .is_empty()
    );
    repo.commit("Move housing").unwrap();
    let after = node(&repo, "mechanical/enclosure.FCStd");
    assert_eq!(before.asset, after.asset);
    assert_eq!(before.revision, after.revision);
    let edges = repo
        .dependencies("controller.kicad_pcb", None, false, false)
        .unwrap()
        .edges;
    assert_eq!(edges[0].id, pin);
    assert_eq!(
        edges[0].target_path.as_deref(),
        Some("mechanical/enclosure.FCStd")
    );
    assert!(repo.graph(None).unwrap().requirements_satisfied);
}

#[test]
fn removed_targets_are_reported_and_can_be_cleaned_up_by_stable_id() {
    let (dir, repo) = setup();
    exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    repo.commit("Pins").unwrap();
    let asset = node(&repo, "housing.FCStd").asset;
    fs::remove_file(dir.path().join("housing.FCStd")).unwrap();
    repo.add(&[".".into()]).unwrap();
    assert_eq!(
        repo.graph(None).unwrap().edges[0].status,
        RequirementStatus::MissingAsset
    );
    let impact = repo.impact("housing.FCStd", None, Some("HEAD")).unwrap();
    assert!(!impact.root_present);
    assert_eq!(impact.changes, ["removed"]);
    assert_eq!(impact.direct.len(), 1);
    repo.remove_dependency("controller.kicad_pcb", &asset.to_string(), None)
        .unwrap();
    assert!(repo.graph(None).unwrap().requirements_satisfied);
    repo.commit("Remove model and its requirement").unwrap();
    repo.fsck().unwrap();
}

#[test]
fn graph_queries_are_cycle_safe_and_report_reverse_and_transitive_edges() {
    let (_dir, repo) = setup();
    for asset in ["housing.FCStd", "controller.kicad_pcb"] {
        repo.set_interface(asset, "contract", "mount.txt").unwrap();
    }
    repo.add_dependency(
        "housing.FCStd",
        "controller.kicad_pcb",
        DependencyRequest::Interface("contract".into()),
    )
    .unwrap();
    repo.add_dependency(
        "controller.kicad_pcb",
        "housing.FCStd",
        DependencyRequest::Interface("contract".into()),
    )
    .unwrap();
    exact(&repo, "firmware.rs", "controller.kicad_pcb");
    repo.commit("Interface cycle").unwrap();
    let graph = repo.graph(None).unwrap();
    assert!(graph.requirements_satisfied);
    assert_eq!(
        repo.dependencies("housing.FCStd", None, false, true)
            .unwrap()
            .edges
            .len(),
        2
    );
    assert_eq!(
        repo.dependencies("housing.FCStd", None, true, true)
            .unwrap()
            .edges
            .len(),
        3
    );
    let impact = repo.impact("housing.FCStd", None, None).unwrap();
    assert_eq!(impact.direct.len(), 1);
    assert_eq!(impact.transitive.len(), 1);
    assert!(
        impact
            .direct
            .iter()
            .chain(&impact.transitive)
            .all(|e| e.asset != impact.root.asset)
    );
    repo.fsck().unwrap();
}

#[test]
fn explicit_incompatible_commits_and_checkouts_require_the_override() {
    let (dir, repo) = setup();
    exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    repo.commit("Pins").unwrap();
    repo.create_ref("valid", "HEAD", true).unwrap();
    fs::write(dir.path().join("housing.FCStd"), "R6").unwrap();
    repo.add(&[".".into()]).unwrap();
    assert!(matches!(
        repo.commit("No implicit override"),
        Err(Error::UnsatisfiedDependencies(_))
    ));
    repo.commit_with_options("Explicit WIP", true).unwrap();
    repo.apply_checkout(repo.plan_checkout("valid").unwrap())
        .unwrap();
    assert!(matches!(
        repo.plan_checkout("main"),
        Err(Error::UnsatisfiedDependencies(_))
    ));
    repo.apply_checkout(repo.plan_checkout_with_options("main", true).unwrap())
        .unwrap();
    assert!(!repo.graph(None).unwrap().requirements_satisfied);
    repo.fsck().unwrap();
}

#[test]
fn historical_queries_and_staging_restore_restore_exact_metadata() {
    let (_dir, repo) = setup();
    repo.create_ref("plain", "HEAD", true).unwrap();
    let update = exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    assert!(repo.graph(Some("HEAD")).unwrap().edges.is_empty());
    assert_eq!(repo.graph(None).unwrap().snapshot, update.snapshot);
    repo.restore(&["controller.kicad_pcb".into()], None, true, false)
        .unwrap();
    assert!(repo.graph(None).unwrap().edges.is_empty());
    exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    repo.commit("Pins").unwrap();
    assert!(repo.graph(Some("plain")).unwrap().edges.is_empty());
    assert_eq!(repo.graph(Some("HEAD")).unwrap().edges.len(), 1);
    let config = fs::read_to_string(repo.backend().root().join(".knot/config.toml")).unwrap();
    assert!(config.contains("format_version = 2"));
    repo.apply_checkout(repo.plan_checkout("plain").unwrap())
        .unwrap();
    assert!(repo.graph(None).unwrap().edges.is_empty());
    repo.fsck().unwrap();
}

#[test]
fn invalid_declarations_fail_without_advancing_staging_or_publishing_bad_objects() {
    let (_dir, repo) = setup();
    let index = repo.status().unwrap().staged_snapshot;
    let count = repo.backend().list_objects().unwrap().len();
    assert!(
        repo.add_dependency(
            "housing.FCStd",
            "housing.FCStd",
            DependencyRequest::Revision(None)
        )
        .is_err()
    );
    assert!(
        repo.add_dependency(
            "controller.kicad_pcb",
            "housing.FCStd",
            DependencyRequest::Interface("missing".into())
        )
        .is_err()
    );
    let foreign = node(&repo, "firmware.rs").revision;
    assert!(
        repo.add_dependency(
            "controller.kicad_pcb",
            "housing.FCStd",
            DependencyRequest::Revision(Some(foreign))
        )
        .is_err()
    );
    assert!(
        repo.set_interface("housing.FCStd", "../bad", "mount.txt")
            .is_err()
    );
    assert_eq!(repo.status().unwrap().staged_snapshot, index);
    assert_eq!(repo.backend().list_objects().unwrap().len(), count);
    repo.fsck().unwrap();
}

#[test]
fn multiple_contracts_update_and_remove_independently_with_missing_export_status() {
    let (_dir, repo) = setup();
    iface(&repo);
    repo.set_interface("housing.FCStd", "thermal", "mount.txt")
        .unwrap();
    repo.add_dependency(
        "controller.kicad_pcb",
        "housing.FCStd",
        DependencyRequest::Interface("thermal".into()),
    )
    .unwrap();
    exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    assert_eq!(repo.graph(None).unwrap().edges.len(), 3);
    repo.remove_dependency("controller.kicad_pcb", "housing.FCStd", Some("thermal"))
        .unwrap();
    assert_eq!(repo.graph(None).unwrap().edges.len(), 2);
    repo.remove_dependency("controller.kicad_pcb", "housing.FCStd", None)
        .unwrap();
    assert!(repo.graph(None).unwrap().edges.is_empty());
    repo.add_dependency(
        "controller.kicad_pcb",
        "housing.FCStd",
        DependencyRequest::Interface("pcb_mount".into()),
    )
    .unwrap();
    repo.remove_interface("housing.FCStd", "pcb_mount").unwrap();
    assert_eq!(
        repo.graph(None).unwrap().edges[0].status,
        RequirementStatus::MissingInterface
    );
    assert!(repo.remove_interface("housing.FCStd", "missing").is_err());
    repo.fsck().unwrap();
}

#[test]
fn repeated_metadata_staging_has_committed_ancestry_and_stable_hashes() {
    let (_dir, repo) = setup();
    let original = node(&repo, "controller.kicad_pcb").revision;
    let first = exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    let second = exact(&repo, "controller.kicad_pcb", "housing.FCStd");
    assert_eq!(first.snapshot, second.snapshot);
    assert_eq!(
        repo.load::<Revision>(&second.revision).unwrap().parents,
        vec![original.clone()]
    );
    repo.remove_dependency("controller.kicad_pcb", "housing.FCStd", None)
        .unwrap();
    assert_eq!(node(&repo, "controller.kicad_pcb").revision, original);
    assert!(repo.status().unwrap().staged.is_empty());
    repo.fsck().unwrap();
}

#[test]
fn fsck_rejects_foreign_exports_and_mislabelled_revision_schemas() {
    let (_dir, repo) = setup();
    let export = repo
        .set_interface("controller.kicad_pcb", "pins", "mount.txt")
        .unwrap()
        .object
        .unwrap();
    let mut revision = repo
        .load::<Revision>(&node(&repo, "housing.FCStd").revision)
        .unwrap();
    revision.interfaces = vec![export];
    let bad = repo
        .backend()
        .put_versioned_object(ObjectKind::Revision, 2, &canonical(&revision).unwrap())
        .unwrap();
    assert!(repo.load::<Revision>(&bad).is_ok());
    assert!(matches!(repo.fsck(), Err(Error::Integrity(_))));
    let wrong_schema = repo
        .backend()
        .put_object(ObjectKind::Revision, &canonical(&revision).unwrap())
        .unwrap();
    assert!(matches!(
        repo.load::<Revision>(&wrong_schema),
        Err(Error::Integrity(_))
    ));
}
