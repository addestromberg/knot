#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::*;
use knot_store::FsBackend;
use std::{fs, path::Path};
use tempfile::{TempDir, tempdir};

fn setup() -> (TempDir, Repository<FsBackend>) {
    let dir = tempdir().unwrap();
    let repo =
        Repository::open(FsBackend::init(dir.path(), Some("Engineer".into())).unwrap()).unwrap();
    (dir, repo)
}
fn commit(repo: &Repository<FsBackend>, message: &str) -> ObjectId {
    repo.add(&[".".into()]).unwrap();
    repo.commit(message).unwrap()
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
fn electromechanical_snapshots_reconstruct_exact_bytes_and_keep_issues_current() {
    let (dir, repo) = setup();
    let old_cad = b"FreeCAD\0housing R1\xff";
    fs::write(dir.path().join("housing.FCStd"), old_cad).unwrap();
    fs::write(dir.path().join("controller.kicad_pcb"), b"(pcb R1)\r\n").unwrap();
    fs::create_dir(dir.path().join("firmware")).unwrap();
    fs::write(dir.path().join("firmware/main.rs"), b"fn main() {}\n").unwrap();
    let first = commit(&repo, "Initial design");
    let s1 = repo.load::<Commit>(&first).unwrap().snapshot;
    repo.create_ref("v1", "HEAD", true).unwrap();
    let issue = repo
        .create_issue("Move mounting holes", "Check PCB fit after moving holes.")
        .unwrap();
    fs::write(dir.path().join("housing.FCStd"), b"FreeCAD\0housing R2\xfe").unwrap();
    fs::write(dir.path().join("controller.kicad_pcb"), b"(pcb R2)\r\n").unwrap();
    let second = commit(&repo, "Update housing and PCB");
    let s2 = repo.load::<Commit>(&second).unwrap().snapshot;
    repo.update_issue(&issue.id.to_string(), IssueAction::Close, Some(&issue.head))
        .unwrap();
    repo.apply_checkout(repo.plan_checkout("v1").unwrap())
        .unwrap();
    assert_eq!(fs::read(dir.path().join("housing.FCStd")).unwrap(), old_cad);
    assert_eq!(repo.status().unwrap().snapshot, Some(s1));
    assert_eq!(repo.issue(&issue.id.to_string()).unwrap().status, "closed");
    repo.apply_checkout(repo.plan_checkout("main").unwrap())
        .unwrap();
    assert_eq!(repo.status().unwrap().snapshot, Some(s2.clone()));
    assert_eq!(repo.history("HEAD", None, 100).unwrap().len(), 2);
    let destination = tempdir().unwrap();
    copy_dir(&dir.path().join(".knot"), &destination.path().join(".knot"));
    let other = Repository::open(FsBackend::open(destination.path()).unwrap()).unwrap();
    other.restore(&[".".into()], None, false, false).unwrap();
    assert_eq!(other.status().unwrap().snapshot, Some(s2));
    for file in ["housing.FCStd", "controller.kicad_pcb", "firmware/main.rs"] {
        assert_eq!(
            fs::read(dir.path().join(file)).unwrap(),
            fs::read(destination.path().join(file)).unwrap()
        );
    }
    other.fsck().unwrap();
    repo.fsck().unwrap();
}

#[test]
fn renames_keep_identity_copies_do_not_and_staging_ancestry_uses_commits() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("old.step"), b"R1").unwrap();
    let first = commit(&repo, "Initial");
    let asset = repo.asset_at("old.step").unwrap();
    let first_snapshot = repo
        .load::<Snapshot>(&repo.load::<Commit>(&first).unwrap().snapshot)
        .unwrap();
    let first_revision = first_snapshot.entries[0].revision.clone();
    repo.move_asset("old.step", "mechanical/new.step", false)
        .unwrap();
    repo.commit("Move asset").unwrap();
    assert_eq!(repo.asset_at("mechanical/new.step").unwrap(), asset);
    fs::copy(
        dir.path().join("mechanical/new.step"),
        dir.path().join("copy.step"),
    )
    .unwrap();
    repo.add(&["copy.step".into()]).unwrap();
    assert_ne!(repo.asset_at("copy.step").unwrap(), asset);
    fs::write(dir.path().join("mechanical/new.step"), b"R2 staged").unwrap();
    repo.add(&["mechanical/new.step".into()]).unwrap();
    fs::write(dir.path().join("mechanical/new.step"), b"R3 staged").unwrap();
    let snapshot = repo.add(&["mechanical/new.step".into()]).unwrap();
    let entries = repo.load::<Snapshot>(&snapshot).unwrap().entries;
    let current = &entries
        .iter()
        .find(|e| e.path == "mechanical/new.step")
        .unwrap()
        .revision;
    assert_eq!(
        repo.load::<Revision>(current).unwrap().parents,
        vec![first_revision]
    );
    repo.commit("Update and copy").unwrap();
    assert_eq!(
        repo.history("HEAD", Some("mechanical/new.step"), 10)
            .unwrap()
            .len(),
        3
    );
    repo.fsck().unwrap();
}

#[test]
fn checkout_protects_dirty_staged_untracked_and_ignored_paths() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    commit(&repo, "First");
    repo.create_ref("v1", "HEAD", true).unwrap();
    fs::write(dir.path().join("a.txt"), "two\n").unwrap();
    commit(&repo, "Second");
    fs::write(dir.path().join("a.txt"), "dirty\n").unwrap();
    assert!(matches!(repo.plan_checkout("v1"), Err(Error::Conflict(_))));
    repo.add(&["a.txt".into()]).unwrap();
    assert!(repo.plan_checkout("v1").is_err());
    repo.restore(&["a.txt".into()], None, true, false).unwrap();
    repo.restore(&["a.txt".into()], None, false, false).unwrap();
    fs::write(dir.path().join("new.txt"), "tracked on main").unwrap();
    commit(&repo, "Add destination");
    repo.apply_checkout(repo.plan_checkout("v1").unwrap())
        .unwrap();
    fs::write(dir.path().join("new.txt"), "precious local data").unwrap();
    fs::write(dir.path().join(".knotignore"), "new.txt\n").unwrap();
    assert!(repo.plan_checkout("main").is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("new.txt")).unwrap(),
        "precious local data"
    );
    assert!(
        repo.status()
            .unwrap()
            .untracked
            .contains(&".knotignore".into())
    );
    assert!(!repo.status().unwrap().untracked.contains(&"new.txt".into()));
}

#[test]
fn checkout_preview_changes_nothing_and_stale_preview_cannot_apply() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("a"), "one").unwrap();
    commit(&repo, "First");
    repo.create_ref("old", "HEAD", true).unwrap();
    fs::write(dir.path().join("a"), "two").unwrap();
    commit(&repo, "Second");
    let plan = repo.plan_checkout("old").unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "two");
    fs::write(dir.path().join("a"), "external edit").unwrap();
    assert!(repo.apply_checkout(plan).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("a")).unwrap(),
        "external edit"
    );
    assert!(
        repo.backend()
            .read_meta("transaction.json")
            .unwrap()
            .is_none()
    );
}

#[test]
fn branches_tags_detached_commits_deletions_and_restore() {
    let (dir, repo) = setup();
    assert!(repo.commit("Empty").is_err());
    fs::write(dir.path().join("a"), "one").unwrap();
    let first = commit(&repo, "First");
    repo.create_ref("feature/new-housing", "HEAD", false)
        .unwrap();
    repo.create_ref("v1", "HEAD", true).unwrap();
    assert!(repo.create_ref("v1", "HEAD", true).is_err());
    assert!(repo.create_ref("MAIN", "HEAD", false).is_err());
    repo.apply_checkout(repo.plan_checkout(&first.to_string()).unwrap())
        .unwrap();
    assert!(repo.status().unwrap().branch.is_none());
    fs::write(dir.path().join("a"), "detached").unwrap();
    let detached = commit(&repo, "Detached");
    assert_eq!(repo.head_id().unwrap(), Some(detached));
    repo.apply_checkout(repo.plan_checkout("main").unwrap())
        .unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "one");
    fs::remove_file(dir.path().join("a")).unwrap();
    repo.add(&[".".into()]).unwrap();
    assert_eq!(repo.status().unwrap().staged[0].kind, "deleted");
    repo.restore(&["a".into()], None, true, false).unwrap();
    repo.restore(&["a".into()], None, false, false).unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "one");
    repo.remove("a", true).unwrap();
    assert!(dir.path().join("a").exists());
    repo.remove("a", false).unwrap();
    repo.commit("Delete").unwrap();
    assert!(!dir.path().join("a").exists());
    repo.apply_checkout(repo.plan_checkout("v1").unwrap())
        .unwrap();
    assert!(dir.path().join("a").exists());
    repo.fsck().unwrap();
}

#[test]
fn file_directory_transitions_checkout_in_both_directions() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("part"), "file").unwrap();
    commit(&repo, "File");
    repo.create_ref("file-state", "HEAD", true).unwrap();
    fs::remove_file(dir.path().join("part")).unwrap();
    fs::create_dir(dir.path().join("part")).unwrap();
    fs::write(dir.path().join("part/inside"), "nested").unwrap();
    commit(&repo, "Directory");
    repo.apply_checkout(repo.plan_checkout("file-state").unwrap())
        .unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("part")).unwrap(), "file");
    repo.apply_checkout(repo.plan_checkout("main").unwrap())
        .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("part/inside")).unwrap(),
        "nested"
    );
}

#[test]
fn issue_events_have_optimistic_updates_links_and_independent_history() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("pcb"), "pcb").unwrap();
    let commit_id = commit(&repo, "PCB");
    let initial = repo
        .create_issue("PCB fit", "Measure mounting holes")
        .unwrap();
    let id = initial.id.to_string();
    let updated = repo
        .update_issue(
            &id,
            IssueAction::Comment {
                body: "Measured 20 mm".into(),
            },
            Some(&initial.head),
        )
        .unwrap();
    assert!(
        repo.update_issue(&id, IssueAction::Close, Some(&initial.head))
            .is_err()
    );
    repo.update_issue(
        &id,
        IssueAction::Edit {
            title: Some("Verify PCB fit".into()),
            body: None,
        },
        Some(&updated.head),
    )
    .unwrap();
    repo.update_issue(
        &id,
        IssueAction::Link {
            target: IssueLink::Commit(commit_id),
        },
        None,
    )
    .unwrap();
    repo.update_issue(
        &id,
        IssueAction::Link {
            target: IssueLink::Asset(repo.asset_at("pcb").unwrap()),
        },
        None,
    )
    .unwrap();
    repo.update_issue(&id, IssueAction::Close, None).unwrap();
    assert!(repo.list_issues(false).unwrap().is_empty());
    assert_eq!(repo.list_issues(true).unwrap().len(), 1);
    let reopened = repo
        .update_issue(&id[..8], IssueAction::Reopen, None)
        .unwrap();
    assert_eq!(reopened.status, "open");
    assert_eq!(reopened.comments.len(), 1);
    assert_eq!(reopened.links.len(), 2);
    assert_eq!(reopened.history.len(), 7);
    assert!(repo.status().unwrap().staged.is_empty());
    repo.fsck().unwrap();
}

#[test]
fn integrity_checks_detect_corrupt_objects_and_unsupported_repository_formats() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("a"), "original").unwrap();
    let snapshot = repo.add(&[".".into()]).unwrap();
    let entry = &repo.load::<Snapshot>(&snapshot).unwrap().entries[0];
    let blob = repo.load::<Revision>(&entry.revision).unwrap().blob;
    let path = dir.path().join(format!(
        ".knot/objects/sha256/{}/{}",
        &blob.as_str()[..2],
        &blob.as_str()[2..]
    ));
    fs::write(path, b"corrupt").unwrap();
    assert!(matches!(repo.fsck(), Err(Error::Integrity(_))));
    assert!(matches!(
        repo.commit("Must not commit corrupt data"),
        Err(Error::Integrity(_))
    ));
    drop(repo);
    fs::write(
        dir.path().join(".knot/config.toml"),
        "format_version = 99\nhash = 'sha256'\n",
    )
    .unwrap();
    assert!(matches!(
        FsBackend::open(dir.path()),
        Err(Error::Integrity(_))
    ));
}

#[test]
fn repository_locks_exclude_concurrent_process_handles() {
    let (dir, repo) = setup();
    assert!(matches!(FsBackend::open(dir.path()), Err(Error::Busy)));
    drop(repo);
    assert!(FsBackend::open(dir.path()).is_ok());
    assert!(FsBackend::init(dir.path(), None).is_err());
}

#[test]
fn large_blobs_stream_and_diff_reports_sizes_without_buffering() {
    let (dir, repo) = setup();
    let contents = vec![0x83; 17 * 1024 * 1024];
    fs::write(dir.path().join("large.stl"), &contents).unwrap();
    commit(&repo, "Large binary");
    repo.create_ref("large", "HEAD", true).unwrap();
    fs::write(dir.path().join("large.stl"), b"small\0").unwrap();
    let diff = repo.diff(false).unwrap();
    assert_eq!(diff[0].before_size, Some(contents.len() as u64));
    assert!(diff[0].patch.is_none());
    commit(&repo, "Small binary");
    repo.apply_checkout(repo.plan_checkout("large").unwrap())
        .unwrap();
    assert_eq!(fs::read(dir.path().join("large.stl")).unwrap(), contents);
    repo.fsck().unwrap();
}

#[cfg(unix)]
#[test]
fn executability_is_versioned_and_symlinks_are_rejected_without_following() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (dir, repo) = setup();
    fs::write(dir.path().join("build"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(dir.path().join("build"), fs::Permissions::from_mode(0o755)).unwrap();
    commit(&repo, "Executable");
    repo.create_ref("executable", "HEAD", true).unwrap();
    fs::set_permissions(dir.path().join("build"), fs::Permissions::from_mode(0o644)).unwrap();
    commit(&repo, "Non-executable");
    repo.apply_checkout(repo.plan_checkout("executable").unwrap())
        .unwrap();
    assert_ne!(
        fs::metadata(dir.path().join("build"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("secret"), "not a project file").unwrap();
    symlink(outside.path(), dir.path().join("escape")).unwrap();
    assert!(repo.add(&["escape/secret".into()]).is_err());
    assert!(repo.backend().file_state("escape/secret", false).is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("secret")).unwrap(),
        "not a project file"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn nonportable_names_and_case_collisions_fail_before_staging() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("literal\\name"), "data").unwrap();
    assert!(repo.add(&[".".into()]).is_err());
    fs::remove_file(dir.path().join("literal\\name")).unwrap();
    fs::write(dir.path().join("a"), "one").unwrap();
    fs::write(dir.path().join("A"), "two").unwrap();
    assert!(repo.add(&[".".into()]).is_err());
    assert!(repo.status().is_err());
}

#[test]
fn rejected_partial_rename_restore_does_not_publish_invalid_snapshots() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("old"), "part").unwrap();
    commit(&repo, "Initial");
    repo.move_asset("old", "new", false).unwrap();
    let count = repo.backend().list_objects().unwrap().len();
    assert!(repo.restore(&["old".into()], None, true, false).is_err());
    assert_eq!(repo.backend().list_objects().unwrap().len(), count);
    repo.fsck().unwrap();
    let preview = repo.restore(&[".".into()], None, true, true).unwrap();
    assert!(preview.staged);
    assert!(preview.staged_snapshot.is_some());
    assert_eq!(preview.changes.len(), 2);
    assert!(dir.path().join("new").exists());
}

#[test]
fn refs_require_canonical_pointers_and_case_only_moves_fail_cleanly() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("Housing"), "part").unwrap();
    let head = commit(&repo, "Initial");
    assert!(matches!(
        repo.move_asset("Housing", "housing", false),
        Err(Error::Conflict(_))
    ));
    assert!(dir.path().join("Housing").exists());
    repo.fsck().unwrap();
    fs::write(dir.path().join(".knot/refs/heads/main"), head.to_string()).unwrap();
    assert!(matches!(repo.fsck(), Err(Error::Integrity(_))));
}

#[test]
fn oversized_issue_is_rejected_before_publishing_a_ref() {
    let (_dir, repo) = setup();
    assert!(matches!(
        repo.create_issue("Large", &"x".repeat(MAX_STRUCTURED_OBJECT_SIZE)),
        Err(Error::Invalid(_))
    ));
    assert!(repo.list_issues(true).unwrap().is_empty());
    repo.fsck().unwrap();
}

#[test]
fn noncanonical_json_and_unsupported_object_schemas_are_rejected() {
    let (dir, repo) = setup();
    let noncanonical = repo
        .backend()
        .put_object(ObjectKind::Snapshot, b"{ \"entries\": [] }")
        .unwrap();
    assert!(matches!(
        repo.load::<Snapshot>(&noncanonical),
        Err(Error::Integrity(_))
    ));
    let valid = repo
        .backend()
        .put_object(ObjectKind::Blob, b"data")
        .unwrap();
    let path = dir.path().join(format!(
        ".knot/objects/sha256/{}/{}",
        &valid.as_str()[..2],
        &valid.as_str()[2..]
    ));
    fs::write(&path, b"knot\0blob\x002\x004\0data").unwrap();
    assert!(matches!(
        repo.backend().verify_blob(&valid),
        Err(Error::Integrity(_))
    ));
    // Malicious length headers must produce an error, never integer overflow.
    fs::write(path, b"knot\0blob\x001\x0018446744073709551615\0").unwrap();
    assert!(matches!(
        repo.backend().verify_blob(&valid),
        Err(Error::Integrity(_))
    ));
}

#[test]
fn equivalent_changes_on_branches_have_identical_snapshot_hashes() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("a"), "a1").unwrap();
    fs::write(dir.path().join("b"), "b1").unwrap();
    commit(&repo, "Base");
    repo.create_ref("equivalent", "HEAD", false).unwrap();
    fs::write(dir.path().join("a"), "a2").unwrap();
    fs::write(dir.path().join("b"), "b2").unwrap();
    let first = commit(&repo, "First ordering");
    repo.apply_checkout(repo.plan_checkout("equivalent").unwrap())
        .unwrap();
    fs::write(dir.path().join("b"), "b2").unwrap();
    repo.add(&["b".into()]).unwrap();
    fs::write(dir.path().join("a"), "a2").unwrap();
    repo.add(&["a".into()]).unwrap();
    let second = repo.commit("Different metadata and staging order").unwrap();
    assert_ne!(first, second);
    assert_eq!(
        repo.load::<Commit>(&first).unwrap().snapshot,
        repo.load::<Commit>(&second).unwrap().snapshot
    );
}

#[test]
fn portable_fixture_has_the_same_full_snapshot_and_commit_hashes_on_every_platform() {
    let (dir, repo) = setup();
    let backend = repo.backend();
    let asset = Asset {
        identity: uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap(),
    };
    let asset = backend
        .put_object(ObjectKind::Asset, &canonical(&asset).unwrap())
        .unwrap();
    let bytes = b"#!/bin/sh\nprintf knot\n";
    let blob = backend.put_object(ObjectKind::Blob, bytes).unwrap();
    let revision = Revision {
        asset,
        blob,
        parents: vec![],
        executable: true,
        dependencies: vec![],
        interfaces: vec![],
    };
    let revision = backend
        .put_object(ObjectKind::Revision, &canonical(&revision).unwrap())
        .unwrap();
    let snapshot = Snapshot {
        entries: vec![SnapshotEntry {
            path: "firmware/build.sh".into(),
            revision,
        }],
    };
    let snapshot = backend
        .put_object(ObjectKind::Snapshot, &canonical(&snapshot).unwrap())
        .unwrap();
    assert_eq!(
        snapshot.as_str(),
        "4e5c159849ac8ca1da0705e1064f49cebc9a3331cb4e78ee88660258a5c81fd8"
    );
    let commit = Commit {
        snapshot: snapshot.clone(),
        parents: vec![],
        message: "Portable fixture".into(),
        author: None,
        timestamp: "1700000000".into(),
        overrides: vec![],
    };
    let commit = backend
        .put_object(ObjectKind::Commit, &canonical(&commit).unwrap())
        .unwrap();
    assert_eq!(
        commit.as_str(),
        "8256f511bdec5083d18c59c5aa4e5815d4496bc0d13f95def10830a2ea42cb49"
    );
    backend
        .write_meta("index", Some(&format!("{snapshot}\n")))
        .unwrap();
    backend
        .write_meta("refs/heads/main", Some(&format!("{commit}\n")))
        .unwrap();
    repo.restore(&[".".into()], None, false, false).unwrap();
    assert_eq!(
        fs::read(dir.path().join("firmware/build.sh")).unwrap(),
        bytes
    );
    assert!(repo.status().unwrap().unstaged.is_empty());
    repo.fsck().unwrap();
}

#[test]
fn initialization_does_not_replace_existing_directories_or_user_files() {
    let dir = tempdir().unwrap();
    fs::create_dir(dir.path().join(".knot")).unwrap();
    fs::write(dir.path().join(".knot/sentinel"), "keep").unwrap();
    assert!(matches!(
        FsBackend::init(dir.path(), None),
        Err(Error::Conflict(_))
    ));
    assert_eq!(
        fs::read_to_string(dir.path().join(".knot/sentinel")).unwrap(),
        "keep"
    );
    let other = tempdir().unwrap();
    let backend = FsBackend::init(other.path(), None).unwrap();
    assert!(other.path().join(".knot/HEAD").is_file());
    assert_eq!(fs::read_dir(other.path()).unwrap().count(), 1);
    drop(backend);
}

#[test]
fn checkout_rejects_untracked_directory_case_collisions_before_journaling() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("base"), "base").unwrap();
    commit(&repo, "Base");
    repo.create_ref("base-state", "HEAD", true).unwrap();
    fs::create_dir(dir.path().join("Mechanical")).unwrap();
    fs::write(dir.path().join("Mechanical/housing"), "CAD").unwrap();
    commit(&repo, "Housing");
    repo.apply_checkout(repo.plan_checkout("base-state").unwrap())
        .unwrap();
    fs::create_dir(dir.path().join("mechanical")).unwrap();
    fs::write(dir.path().join("mechanical/local"), "precious").unwrap();
    assert!(matches!(
        repo.plan_checkout("main"),
        Err(Error::Conflict(_))
    ));
    assert!(
        repo.backend()
            .read_meta("transaction.json")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("mechanical/local")).unwrap(),
        "precious"
    );
}

#[test]
fn empty_untracked_directories_can_be_replaced_without_touching_other_files() {
    let (dir, repo) = setup();
    fs::write(dir.path().join("base"), "base").unwrap();
    commit(&repo, "Base");
    repo.create_ref("base-state", "HEAD", true).unwrap();
    fs::write(dir.path().join("part"), "CAD").unwrap();
    commit(&repo, "Part");
    repo.apply_checkout(repo.plan_checkout("base-state").unwrap())
        .unwrap();
    fs::create_dir(dir.path().join("part")).unwrap();
    repo.apply_checkout(repo.plan_checkout("main").unwrap())
        .unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("part")).unwrap(), "CAD");
    repo.fsck().unwrap();
}
