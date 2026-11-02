#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::*;
use knot_store::*;
use std::{fs, io::Cursor};
use tempfile::tempdir;

fn review(repo: &Repository<FsBackend>) {
    repo.record_review(
        repo.review_subject("rod", Some("pipe")).unwrap(),
        ReviewOutcome::Passed,
        "Engineer",
        "Measured fit",
        "bench",
        &[],
    )
    .unwrap();
    repo.record_review(
        repo.test_subject("fit").unwrap(),
        ReviewOutcome::Passed,
        "Engineer",
        "Fit test passed",
        "bench",
        &[],
    )
    .unwrap();
}

#[test]
fn two_computers_exchange_exact_snapshots_issues_reviews_releases_and_large_blobs() {
    let source_dir = tempdir().unwrap();
    let peer_dir = tempdir().unwrap();
    let second_dir = tempdir().unwrap();
    let source =
        Repository::open(FsBackend::init(source_dir.path(), Some("Engineer".into())).unwrap())
            .unwrap();
    for (name, contents) in [
        ("pipe", "pipe R1"),
        ("rod", "rod"),
        ("fit.md", "Measure sliding clearance"),
    ] {
        fs::write(source_dir.path().join(name), contents).unwrap();
    }
    fs::write(
        source_dir.path().join("large.bin"),
        vec![71; 18 * 1024 * 1024],
    )
    .unwrap();
    source.add(&[".".into()]).unwrap();
    source
        .add_dependency("rod", "pipe", DependencyRequest::Revision(None))
        .unwrap();
    source
        .define_test("fit", "fit.md", &["pipe".into(), "rod".into()], "Fit check")
        .unwrap();
    source.commit("Initial assembly").unwrap();
    source.create_ref("v1", "HEAD", true).unwrap();
    review(&source);
    source.create_release("v1", "Approver", "Measured").unwrap();
    let issue = source
        .create_issue("Watch clearance", "Check future pipe changes")
        .unwrap();
    drop(FsBackend::init_bare(peer_dir.path(), None).unwrap());
    remote_add(&source, "origin", peer_dir.path().to_str().unwrap()).unwrap();
    let sent = push(&source, "origin", None).unwrap();
    assert!(sent.objects > 0);
    let destination = second_dir.path().join("project");
    clone_repository(peer_dir.path().to_str().unwrap(), &destination, None, false).unwrap();
    let second = Repository::open(FsBackend::open(&destination).unwrap()).unwrap();
    assert_eq!(source.head_id().unwrap(), second.head_id().unwrap());
    assert_eq!(
        source.status().unwrap().staged_snapshot,
        second.status().unwrap().staged_snapshot
    );
    assert_eq!(
        fs::metadata(destination.join("large.bin")).unwrap().len(),
        18 * 1024 * 1024
    );
    assert_eq!(
        second.issue(&issue.id.to_string()).unwrap().title,
        "Watch clearance"
    );
    assert_eq!(second.releases().unwrap().len(), 1);
    assert!(
        second
            .verification(None)
            .unwrap()
            .dependencies
            .iter()
            .all(|e| e.state == VerificationState::Verified)
    );
    assert_eq!(fetch(&second, "origin").unwrap().objects, 0);

    fs::write(source_dir.path().join("pipe"), "pipe R2").unwrap();
    source.add(&["pipe".into()]).unwrap();
    source
        .add_dependency("rod", "pipe", DependencyRequest::Revision(None))
        .unwrap();
    source.commit("Update pipe").unwrap();
    review(&source);
    source
        .update_issue(
            &issue.id.to_string(),
            IssueAction::Comment {
                body: "R2 measured".into(),
            },
            None,
        )
        .unwrap();
    push(&source, "origin", None).unwrap();
    let before = second.head_id().unwrap();
    fetch(&second, "origin").unwrap();
    assert_eq!(second.head_id().unwrap(), before);
    assert_eq!(
        fs::read_to_string(destination.join("pipe")).unwrap(),
        "pipe R1"
    );
    pull(&second, "origin", false).unwrap();
    assert_eq!(source.head_id().unwrap(), second.head_id().unwrap());
    assert_eq!(
        fs::read_to_string(destination.join("pipe")).unwrap(),
        "pipe R2"
    );
    assert_eq!(
        second.verification(None).unwrap().tests[0].state,
        VerificationState::Verified
    );
    fs::write(destination.join("notes.md"), "Reviewed on second computer").unwrap();
    second.add(&["notes.md".into()]).unwrap();
    second.commit("Second computer note").unwrap();
    push(&second, "origin", None).unwrap();
    pull(&source, "origin", false).unwrap();
    assert_eq!(source.head_id().unwrap(), second.head_id().unwrap());
    assert_eq!(
        fs::read_to_string(source_dir.path().join("notes.md")).unwrap(),
        "Reviewed on second computer"
    );
    source.fsck().unwrap();
    second.fsck().unwrap();
    let bare = Repository::open(FsBackend::open(peer_dir.path()).unwrap()).unwrap();
    assert!(bare.backend().is_bare());
    assert!(bare.status().unwrap().staged.is_empty());
    assert!(bare.status().unwrap().unstaged.is_empty());
    assert!(!peer_dir.path().join("pipe").exists());
    bare.fsck().unwrap();
}

#[test]
fn divergent_push_and_pull_preserve_refs_and_working_files() {
    let dir = tempdir().unwrap();
    let bare_path = dir.path().join("peer");
    drop(FsBackend::init_bare(&bare_path, None).unwrap());
    let a_path = dir.path().join("a");
    let a = Repository::open(FsBackend::init(&a_path, None).unwrap()).unwrap();
    fs::write(a_path.join("file"), "base").unwrap();
    a.add(&[".".into()]).unwrap();
    a.commit("Base").unwrap();
    remote_add(&a, "origin", bare_path.to_str().unwrap()).unwrap();
    push(&a, "origin", None).unwrap();
    let b_path = dir.path().join("b");
    clone_repository(bare_path.to_str().unwrap(), &b_path, None, false).unwrap();
    let b = Repository::open(FsBackend::open(&b_path).unwrap()).unwrap();
    fs::write(a_path.join("file"), "A").unwrap();
    a.add(&["file".into()]).unwrap();
    a.commit("A branch").unwrap();
    push(&a, "origin", None).unwrap();
    fs::write(b_path.join("file"), "B").unwrap();
    b.add(&["file".into()]).unwrap();
    b.commit("B branch").unwrap();
    let before = b.head_id().unwrap();
    assert_eq!(push(&b, "origin", None).unwrap_err().code(), "conflict");
    assert!(pull(&b, "origin", false).is_err());
    assert_eq!(b.head_id().unwrap(), before);
    assert_eq!(fs::read_to_string(b_path.join("file")).unwrap(), "B");
    let peer = Repository::open(FsBackend::open(&bare_path).unwrap()).unwrap();
    assert_eq!(peer.head_id().unwrap(), a.head_id().unwrap());
    peer.fsck().unwrap();
}

#[test]
fn object_import_rejects_corruption_and_truncation_before_publication() {
    let a = tempdir().unwrap();
    let b = tempdir().unwrap();
    let source = FsBackend::init(a.path(), None).unwrap();
    let destination = FsBackend::init(b.path(), None).unwrap();
    let id = source
        .put_object(ObjectKind::Blob, b"immutable binary\0contents")
        .unwrap();
    let mut raw = Vec::new();
    source.export_object(&id, &mut raw).unwrap();
    let count = destination.list_objects().unwrap().len();
    let mut corrupt = raw.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    assert!(
        destination
            .import_object(&id, raw.len() as u64, &mut Cursor::new(corrupt))
            .is_err()
    );
    assert_eq!(destination.list_objects().unwrap().len(), count);
    assert!(
        destination
            .import_object(
                &id,
                raw.len() as u64,
                &mut Cursor::new(&raw[..raw.len() - 1])
            )
            .is_err()
    );
    assert_eq!(destination.list_objects().unwrap().len(), count);
    destination
        .import_object(&id, raw.len() as u64, &mut Cursor::new(&raw))
        .unwrap();
    assert_eq!(
        destination.get_object(&id).unwrap().1,
        b"immutable binary\0contents"
    );
}

#[test]
fn checked_out_peer_and_dirty_pull_are_protected() {
    let dir = tempdir().unwrap();
    let a_path = dir.path().join("a");
    let a = Repository::open(FsBackend::init(&a_path, None).unwrap()).unwrap();
    fs::write(a_path.join("file"), "one").unwrap();
    a.add(&[".".into()]).unwrap();
    a.commit("One").unwrap();
    drop(a);
    let b_path = dir.path().join("b");
    clone_repository(a_path.to_str().unwrap(), &b_path, None, false).unwrap();
    let b = Repository::open(FsBackend::open(&b_path).unwrap()).unwrap();
    fs::write(b_path.join("file"), "two").unwrap();
    b.add(&["file".into()]).unwrap();
    b.commit("Two").unwrap();
    assert!(push(&b, "origin", None).is_err());
    assert_eq!(fs::read_to_string(a_path.join("file")).unwrap(), "one");
    drop(b);
    let bare_path = dir.path().join("bare");
    drop(FsBackend::init_bare(&bare_path, None).unwrap());
    let a = Repository::open(FsBackend::open(&a_path).unwrap()).unwrap();
    remote_add(&a, "origin", bare_path.to_str().unwrap()).unwrap();
    push(&a, "origin", None).unwrap();
    let c_path = dir.path().join("c");
    clone_repository(bare_path.to_str().unwrap(), &c_path, None, false).unwrap();
    let c = Repository::open(FsBackend::open(&c_path).unwrap()).unwrap();
    fs::write(a_path.join("file"), "new peer data").unwrap();
    a.add(&["file".into()]).unwrap();
    a.commit("New").unwrap();
    push(&a, "origin", None).unwrap();
    fs::write(c_path.join("file"), "unsaved local work").unwrap();
    let before = c.head_id().unwrap();
    assert!(pull(&c, "origin", false).is_err());
    assert_eq!(c.head_id().unwrap(), before);
    assert_eq!(
        fs::read_to_string(c_path.join("file")).unwrap(),
        "unsaved local work"
    );
}

#[test]
fn compare_and_swap_and_immutable_references_are_enforced() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    fs::write(dir.path().join("file"), "one").unwrap();
    repo.add(&[".".into()]).unwrap();
    let first = repo.commit("One").unwrap();
    repo.create_ref("v1", "HEAD", true).unwrap();
    fs::write(dir.path().join("file"), "two").unwrap();
    repo.add(&[".".into()]).unwrap();
    let second = repo.commit("Two").unwrap();
    assert!(
        repo.publish_references(
            &[ReferenceUpdate {
                name: "refs/heads/main".into(),
                before: Some(first.clone()),
                after: second.clone()
            }],
            None,
            false
        )
        .is_err()
    );
    assert!(
        repo.publish_references(
            &[ReferenceUpdate {
                name: "refs/tags/v1".into(),
                before: Some(first.clone()),
                after: second
            }],
            None,
            false
        )
        .is_err()
    );
    assert_eq!(repo.resolve("v1").unwrap(), first);
}
