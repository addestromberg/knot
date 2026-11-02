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
fn local_ref_creation_enforces_the_same_portable_components_as_clone_and_fsck() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("source");
    let repo = Repository::open(FsBackend::init(&root, None).unwrap()).unwrap();
    repo.write_asset("part", b"A").unwrap();
    let head = repo.commit("Initial").unwrap();
    repo.create_ref("Feature/a", "HEAD", false).unwrap();
    let before = repo.reference_inventory().unwrap();
    assert_eq!(
        repo.create_ref("feature/b", "HEAD", false)
            .unwrap_err()
            .code(),
        "conflict"
    );
    assert_eq!(repo.reference_inventory().unwrap(), before);
    repo.create_ref("Feature/b", "HEAD", false).unwrap();
    repo.create_ref("Version/a", "HEAD", true).unwrap();
    assert_eq!(
        repo.create_ref("version/b", "HEAD", true)
            .unwrap_err()
            .code(),
        "conflict"
    );
    repo.create_ref("Version/b", "HEAD", true).unwrap();
    repo.create_release("Approved/a", "Engineer", "Reviewed")
        .unwrap();
    assert_eq!(
        repo.create_release("approved/b", "Engineer", "Reviewed")
            .unwrap_err()
            .code(),
        "conflict"
    );
    repo.create_release("Approved/b", "Engineer", "Reviewed")
        .unwrap();
    repo.fsck().unwrap();
    let expected = repo.reference_inventory().unwrap();
    drop(repo);
    let copy = dir.path().join("copy");
    clone_repository(root.to_str().unwrap(), &copy, None, false).unwrap();
    let cloned = Repository::open(FsBackend::open(copy).unwrap()).unwrap();
    assert_eq!(cloned.reference_inventory().unwrap(), expected);
    cloned.fsck().unwrap();
    // Bypass public creation to model malformed metadata on disk. Only a
    // case-sensitive filesystem can represent both directory spellings.
    cloned
        .backend()
        .write_meta("refs/heads/feature/c", Some(&format!("{head}\n")))
        .unwrap();
    if cloned
        .backend()
        .list_meta("refs/heads/")
        .unwrap()
        .iter()
        .any(|n| n == "refs/heads/feature/c")
    {
        assert!(cloned.fsck().is_err());
    }
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
fn interrupted_uploads_roll_back_every_frame_boundary_and_push_can_retry() {
    let dir = tempdir().unwrap();
    let source =
        Repository::open(FsBackend::init(dir.path().join("source"), None).unwrap()).unwrap();
    source.write_asset("part", b"new part").unwrap();
    source.commit("Initial").unwrap();
    let objects = source.backend().list_objects().unwrap();
    let frames = objects
        .iter()
        .map(|id| ObjectFrame {
            id: id.clone(),
            len: source.backend().object_size(id).unwrap(),
        })
        .collect::<Vec<_>>();
    let updates = source
        .reference_inventory()
        .unwrap()
        .into_iter()
        .map(|(name, after)| ReferenceUpdate {
            name,
            before: None,
            after,
        })
        .collect::<Vec<_>>();
    let mut bytes = Vec::new();
    let mut boundaries = vec![0];
    for id in &objects {
        source.backend().export_object(id, &mut bytes).unwrap();
        boundaries.push(bytes.len());
    }
    for (number, boundary) in boundaries[..objects.len()].iter().enumerate() {
        let path = dir.path().join(format!("peer-{number}"));
        let peer = Repository::open(FsBackend::init_bare(&path, None).unwrap()).unwrap();
        // Existing unrelated objects must survive a failed upload.
        let retained = peer
            .backend()
            .put_object(ObjectKind::Blob, b"existing orphan")
            .unwrap();
        let existing = peer.backend().list_objects().unwrap();
        let mut stream = serde_json::to_vec(
            &serde_json::json!({"operation":"put","objects":frames,"updates":updates}),
        )
        .unwrap();
        stream.push(b'\n');
        stream.extend_from_slice(&bytes[..*boundary]);
        let mut output = Vec::new();
        serve_repository(&peer, &mut Cursor::new(stream), &mut output, true).unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses[0]["data"]["ready"], true);
        assert_eq!(responses[1]["ok"], false);
        assert!(
            responses[1]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("truncated object transfer")
        );
        assert_eq!(peer.backend().list_objects().unwrap(), existing);
        assert!(peer.head_id().unwrap().is_none());
        assert!(peer.backend().read_meta("incoming.json").unwrap().is_none());
        peer.fsck().unwrap();
        drop(peer);
        let name = format!("peer-{number}");
        remote_add(&source, &name, path.to_str().unwrap()).unwrap();
        push(&source, &name, None).unwrap();
        let peer = Repository::open(FsBackend::open(&path).unwrap()).unwrap();
        assert_eq!(peer.head_id().unwrap(), source.head_id().unwrap());
        assert_eq!(
            peer.backend().get_object(&retained).unwrap().1,
            b"existing orphan"
        );
        peer.fsck().unwrap();
    }
}

#[test]
fn a_complete_batch_with_missing_graph_references_is_rolled_back() {
    let dir = tempdir().unwrap();
    let source =
        Repository::open(FsBackend::init(dir.path().join("source"), None).unwrap()).unwrap();
    source.write_asset("part", b"new part").unwrap();
    let commit = source.commit("Initial").unwrap();
    let peer =
        Repository::open(FsBackend::init_bare(dir.path().join("peer"), None).unwrap()).unwrap();
    let existing = peer.backend().list_objects().unwrap();
    let mut stream = serde_json::to_vec(&serde_json::json!({"operation":"put","objects":[{"id":commit,"len":source.backend().object_size(&commit).unwrap()}],"updates":[]})).unwrap();
    stream.push(b'\n');
    source
        .backend()
        .export_object(&commit, &mut stream)
        .unwrap();
    let mut output = Vec::new();
    serve_repository(&peer, &mut Cursor::new(stream), &mut output, true).unwrap();
    assert!(
        String::from_utf8(output)
            .unwrap()
            .contains("missing object")
    );
    assert_eq!(peer.backend().list_objects().unwrap(), existing);
    peer.fsck().unwrap();
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
