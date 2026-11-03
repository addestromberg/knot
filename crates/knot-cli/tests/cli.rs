#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::tempdir;

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_knot"))
        .arg("-C")
        .arg(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}
fn ok(root: &Path, args: &[&str]) -> Value {
    let result = run(root, args);
    assert!(
        result.status.success(),
        "{args:?}: stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let data: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(data["schema_version"], 1);
    assert_eq!(data["ok"], true);
    assert!(result.stderr.is_empty());
    data["data"].clone()
}

fn human(root: &Path, args: &[&str]) -> String {
    let result = Command::new(env!("CARGO_BIN_EXE_knot"))
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn knotignore_applies_to_cli_status_and_subdirectory_staging() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::create_dir(root.join("cad")).unwrap();
    for (path, contents) in [
        (".knotignore", "*.bak\n"),
        ("cad/.knotignore", "*.stl\n!reference.stl\n"),
        ("cad/model.step", "Model"),
        ("cad/output.stl", "Generated mesh"),
        ("cad/reference.stl", "Reference mesh"),
        ("cad/backup.bak", "Backup"),
    ] {
        fs::write(root.join(path), contents).unwrap();
    }
    let cad = root.join("cad");
    assert_eq!(
        ok(&cad, &["status"])["untracked"],
        json!([
            ".knotignore",
            "cad/.knotignore",
            "cad/model.step",
            "cad/reference.stl"
        ])
    );
    assert!(!run(&cad, &["add", "output.stl"]).status.success());
    ok(&cad, &["add", "."]);
    let staged = ok(root, &["status"]);
    assert_eq!(staged["staged"].as_array().unwrap().len(), 3);
    assert_eq!(staged["untracked"], json!([".knotignore"]));
    ok(root, &["add", ".knotignore"]);
    ok(root, &["commit", "-m", "Ignore generated meshes"]);
    assert_eq!(ok(root, &["status"])["untracked"], json!([]));
    fs::write(cad.join(".knotignore"), "").unwrap();
    assert_eq!(
        ok(&cad, &["status"])["untracked"],
        json!(["cad/output.stl"])
    );
    ok(root, &["fsck"]);
}

#[test]
fn release_checkout_preserves_a_same_spelled_branch() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::write(root.join("part"), "A").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Initial"]);
    let first = ok(root, &["status"])["head"].clone();
    ok(root, &["release", "create", "v1", "--approver", "Engineer"]);
    fs::write(root.join("part"), "B").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Newer"]);
    ok(root, &["branch", "release/v1"]);
    let branches = ok(root, &["branch"]);
    ok(root, &["checkout", "release/v1"]);
    let status = ok(root, &["status"]);
    assert!(status["branch"].is_null());
    assert_eq!(status["head"], first);
    assert_eq!(ok(root, &["branch"]), branches);
    assert_eq!(fs::read(root.join("part")).unwrap(), b"A");
}

#[test]
fn killed_uploads_recover_on_open_and_push_retries_without_losing_existing_objects() {
    use knot_core::{Backend, Repository};
    use knot_store::{FsBackend, push, remote_add};
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let dir = tempdir().unwrap();
    let source =
        Repository::open(FsBackend::init(dir.path().join("source"), None).unwrap()).unwrap();
    source.write_asset("part", b"transfer contents").unwrap();
    source.commit("Initial").unwrap();
    let ids = source.backend().list_objects().unwrap();
    let frames = ids
        .iter()
        .map(|id| json!({"id":id,"len":source.backend().object_size(id).unwrap()}))
        .collect::<Vec<_>>();
    let updates = source
        .reference_inventory()
        .unwrap()
        .into_iter()
        .map(|(name, after)| json!({"name":name,"before":null,"after":after}))
        .collect::<Vec<_>>();
    for count in 1..ids.len() {
        let peer_path = dir.path().join(format!("peer-{count}"));
        let peer = FsBackend::init_bare(&peer_path, None).unwrap();
        let retained = peer
            .put_object(knot_core::ObjectKind::Blob, b"retained orphan")
            .unwrap();
        let existing = peer.list_objects().unwrap();
        drop(peer);
        let mut child = Command::new(env!("CARGO_BIN_EXE_knot"))
            .arg("-C")
            .arg(&peer_path)
            .args(["serve", "--stdio", "--allow-writes"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        writeln!(
            input,
            "{}",
            json!({"operation":"put","objects":frames,"updates":updates})
        )
        .unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["data"]["ready"],
            true
        );
        for id in &ids[..count] {
            source.backend().export_object(id, &mut input).unwrap();
        }
        input.flush().unwrap();
        let object_paths = ids[..count]
            .iter()
            .map(|id| {
                let id = id.as_str();
                peer_path
                    .join(".knot/objects/sha256")
                    .join(&id[..2])
                    .join(&id[2..])
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !peer_path.join(".knot/incoming.json").exists()
            || object_paths.iter().any(|path| !path.exists())
        {
            assert!(
                Instant::now() < deadline,
                "receiver did not import complete frame"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Abrupt termination skips ObjectImport::drop, exercising the persisted
        // rollback record rather than ordinary EOF/error cleanup.
        child.kill().unwrap();
        child.wait().unwrap();
        drop(input);
        assert!(peer_path.join(".knot/incoming.json").exists());
        ok(&peer_path, &["fsck"]);
        let peer = Repository::open(FsBackend::open(&peer_path).unwrap()).unwrap();
        assert_eq!(peer.backend().list_objects().unwrap(), existing);
        assert!(peer.head_id().unwrap().is_none());
        assert_eq!(
            peer.backend().get_object(&retained).unwrap().1,
            b"retained orphan"
        );
        drop(peer);
        let name = format!("peer-{count}");
        remote_add(&source, &name, peer_path.to_str().unwrap()).unwrap();
        push(&source, &name, None).unwrap();
        let peer = Repository::open(FsBackend::open(&peer_path).unwrap()).unwrap();
        assert_eq!(peer.head_id().unwrap(), source.head_id().unwrap());
        peer.fsck().unwrap();
    }
}

#[test]
fn override_guards_are_per_dependency_and_commit_history_marks_accepted_overrides() {
    use std::io::Write;
    use std::process::Stdio;
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    for name in ["pipe", "rod", "assembly"] {
        fs::write(root.join(name), name).unwrap();
    }
    ok(root, &["add", "."]);
    ok(root, &["depend", "add", "rod", "--on", "pipe"]);
    ok(root, &["depend", "add", "assembly", "--on", "rod"]);
    ok(root, &["commit", "-m", "Initial"]);
    fs::write(root.join("pipe"), "new pipe").unwrap();
    ok(root, &["add", "pipe"]);
    let before = ok(root, &["status"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_knot"))
        .arg("-C")
        .arg(root)
        .arg("rape")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"n\nn\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("[y/N]")
            .count(),
        2
    );
    assert_eq!(
        ok(root, &["status"])["staged_snapshot"],
        before["staged_snapshot"]
    );
    let overridden = ok(
        root,
        &[
            "rape",
            "--forced",
            "--reviewer",
            "Operator",
            "--notes",
            "Accept selected revisions",
        ],
    );
    assert_eq!(overridden["accepted"], 2);
    assert_eq!(ok(root, &["status"])["requirements_satisfied"], true);
    assert!(human(root, &["status"]).contains("verified (raped)"));
    ok(root, &["commit", "-m", "Accept new assembly"]);
    let history = ok(root, &["log"]);
    assert_eq!(
        history[0]["commit"]["overrides"].as_array().unwrap().len(),
        2
    );
    assert!(human(root, &["log"]).contains("(raped)"));
    fs::write(root.join("notes.md"), "Unrelated note").unwrap();
    ok(root, &["add", "notes.md"]);
    ok(root, &["commit", "-m", "Keep accepted configuration"]);
    assert_eq!(
        ok(root, &["log"])[0]["commit"]["overrides"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    ok(root, &["fsck"]);
}

#[test]
fn cli_manual_validation_release_and_local_peer_workflow() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    ok(&source, &["init"]);
    for (name, body) in [("pipe", "pipe"), ("rod", "rod"), ("fit.md", "Measure fit")] {
        fs::write(source.join(name), body).unwrap();
    }
    ok(&source, &["add", "."]);
    ok(&source, &["depend", "add", "rod", "--on", "pipe"]);
    ok(
        &source,
        &[
            "tests",
            "define",
            "fit",
            "--procedure",
            "fit.md",
            "--input",
            "pipe",
            "--input",
            "rod",
        ],
    );
    ok(&source, &["commit", "-m", "Tested assembly"]);
    assert_eq!(
        run(&source, &["release", "create", "v1", "--approver", "A"])
            .status
            .code(),
        Some(3)
    );
    ok(
        &source,
        &[
            "verify",
            "rod",
            "--on",
            "pipe",
            "--reviewer",
            "Engineer",
            "--notes",
            "Measured clearance",
        ],
    );
    ok(
        &source,
        &[
            "validate",
            "fit",
            "--record",
            "passed",
            "--reviewer",
            "Engineer",
            "--notes",
            "Fit passed",
        ],
    );
    ok(&source, &["release", "create", "v1", "--approver", "A"]);
    let peer = root.join("peer");
    ok(root, &["init", peer.to_str().unwrap(), "--bare"]);
    ok(
        &source,
        &["remote", "add", "origin", peer.to_str().unwrap()],
    );
    ok(&source, &["push"]);
    let copy = root.join("copy");
    ok(
        root,
        &["clone", peer.to_str().unwrap(), copy.to_str().unwrap()],
    );
    assert_eq!(
        ok(&copy, &["status"])["head"],
        ok(&source, &["status"])["head"]
    );
    assert_eq!(fs::read_to_string(copy.join("pipe")).unwrap(), "pipe");
    ok(&copy, &["checkout", "release/v1", "--validated"]);
    assert_eq!(ok(&copy, &["release", "list"]).as_array().unwrap().len(), 1);
    ok(&copy, &["fsck"]);
}

#[test]
fn ssh_endpoint_advertises_framed_objects_and_rejects_unauthorized_uploads() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::Stdio;
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::write(root.join("file"), "contents").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "One"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_knot"))
        .arg("-C")
        .arg(root)
        .args(["serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    input.write_all(b"{\"operation\":\"advertise\"}\n").unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let inventory: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(inventory["ok"], true);
    let object = &inventory["data"]["objects"][0];
    writeln!(input, "{}", json!({"operation":"get","objects":[object]})).unwrap();
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let frames: Value = serde_json::from_str(&line).unwrap();
    let len = frames["data"][0]["len"].as_u64().unwrap();
    let mut raw = vec![0; len as usize];
    output.read_exact(&mut raw).unwrap();
    assert!(raw.starts_with(b"knot\0"));
    input
        .write_all(b"{\"operation\":\"put\",\"objects\":[],\"updates\":[]}\n")
        .unwrap();
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let denied: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(denied["ok"], false);
    assert!(
        denied["error"]["message"]
            .as_str()
            .unwrap()
            .contains("read-only")
    );
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn writable_peer_endpoint_streams_objects_and_publishes_refs_after_verification() {
    use knot_core::{Backend, Repository};
    use knot_store::FsBackend;
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    let dir = tempdir().unwrap();
    let source_path = dir.path().join("source");
    let destination = dir.path().join("peer");
    ok(dir.path(), &["init", source_path.to_str().unwrap()]);
    ok(
        dir.path(),
        &["init", destination.to_str().unwrap(), "--bare"],
    );
    fs::write(source_path.join("binary"), b"engineering\0object\xff").unwrap();
    ok(&source_path, &["add", "."]);
    ok(&source_path, &["commit", "-m", "Transfer fixture"]);
    let source = Repository::open(FsBackend::open(&source_path).unwrap()).unwrap();
    let ids = source.backend().list_objects().unwrap();
    let frames = ids
        .iter()
        .map(|id| json!({"id":id,"len":source.backend().object_size(id).unwrap()}))
        .collect::<Vec<_>>();
    let updates = source
        .reference_inventory()
        .unwrap()
        .into_iter()
        .map(|(name, id)| json!({"name":name,"before":null,"after":id}))
        .collect::<Vec<_>>();
    let mut child = Command::new(env!("CARGO_BIN_EXE_knot"))
        .arg("-C")
        .arg(&destination)
        .args(["serve", "--stdio", "--allow-writes"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(
        input,
        "{}",
        json!({"operation":"put","objects":frames,"updates":updates})
    )
    .unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["data"]["ready"],
        true
    );
    for id in &ids {
        source.backend().export_object(id, &mut input).unwrap();
    }
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    drop(input);
    assert!(child.wait().unwrap().success());
    let peer = Repository::open(FsBackend::open(&destination).unwrap()).unwrap();
    assert_eq!(peer.head_id().unwrap(), source.head_id().unwrap());
    assert!(!destination.join("binary").exists());
    peer.fsck().unwrap();
}

#[test]
fn status_summarizes_by_default_and_verbose_displays_the_staged_graph() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    let empty = human(root, &["status"]);
    assert!(empty.contains("On branch main · no commits yet"));
    assert!(empty.contains("No tracked files"));
    for (path, body) in [("pipe.step", "pipe R1"), ("rod.step", "rod R1")] {
        fs::write(root.join(path), body).unwrap();
    }
    ok(root, &["add", "."]);
    ok(root, &["depend", "add", "rod.step", "--on", "pipe.step"]);
    ok(root, &["commit", "-m", "Rod and pipe"]);
    let before = ok(root, &["status"]);
    let clean = human(root, &["status"]);
    assert!(clean.contains("Working tree clean"));
    assert!(clean.contains("Files: 2 tracked · 2 unchanged · 0 staged · 0 unstaged · 0 untracked"));
    assert!(clean.contains("Dependencies: 1 satisfied · 0 conflicts"));
    assert!(clean.contains("Verification: 0 verified · 1 unverified"));
    assert!(!clean.contains("pipe.step"));
    let verbose = human(root, &["status", "--verbose"]);
    assert!(verbose.contains("Dependency graph (staged; parent depends on child)"));
    assert!(verbose.contains("#2 rod.step @"));
    assert!(verbose.contains("└── #1 pipe.step @"));
    assert!(verbose.contains("[matches]"));
    assert!(verbose.contains("2 tracked files · 1 dependency · 0 conflicts"));
    assert!(!clean.contains('\u{1b}'));
    assert_eq!(ok(root, &["status"]), before);

    fs::write(root.join("pipe.step"), "pipe with mount").unwrap();
    fs::write(root.join("notes.txt"), "review mounting clearance").unwrap();
    let unstaged = human(root, &["status"]);
    assert!(unstaged.contains("1 unstaged · 1 untracked"));
    assert!(!unstaged.contains("notes.txt"));
    let verbose = human(root, &["status", "-v"]);
    assert!(verbose.contains("Unstaged changes:"));
    assert!(verbose.contains("Untracked files:\n  ?  notes.txt"));
    assert!(verbose.contains("[unstaged modified]"));
    assert!(unstaged.contains("unstaged edits are not included in dependency checks"));
    assert_eq!(ok(root, &["status"])["requirements_satisfied"], true);

    ok(root, &["add", "pipe.step"]);
    let staged = human(root, &["status"]);
    assert!(staged.contains("1 staged · 0 unstaged"));
    let verbose = human(root, &["status", "--verbose"]);
    assert!(verbose.contains("Staged changes:"));
    assert!(verbose.contains("[staged]"));
    assert!(staged.contains("[REVISION MISMATCH]"));
    assert!(staged.contains("1 conflict"));
    assert!(staged.contains("unsatisfied requirements block ordinary commits"));
    assert!(!staged.contains("[unstaged modified]"));
    let data = ok(root, &["status"]);
    assert_eq!(data["head"], before["head"]);
    assert_eq!(data["requirements_satisfied"], false);
    assert_eq!(data["dependency_graph"], ok(root, &["graph"]));
    assert_eq!(
        data["dependency_conflicts"][0],
        data["dependency_graph"]["edges"][0]
    );
}

#[test]
fn graph_renders_shared_dependencies_once_and_missing_targets_explicitly() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    for name in ["pipe.step", "rod.step", "sleeve.step", "readme.txt"] {
        fs::write(root.join(name), name).unwrap();
    }
    ok(root, &["add", "."]);
    for owner in ["rod.step", "sleeve.step"] {
        ok(root, &["depend", "add", owner, "--on", "pipe.step"]);
    }
    ok(root, &["commit", "-m", "Shared pipe dependency"]);
    let output = human(root, &["graph"]);
    for name in ["pipe.step", "rod.step", "sleeve.step", "readme.txt"] {
        assert_eq!(output.matches(name).count(), 1, "{output}");
    }
    assert!(output.contains("↳ #1 (already shown)"));
    assert_eq!(output.matches("[matches]").count(), 2);
    ok(root, &["rm", "pipe.step"]);
    let missing = human(root, &["status"]);
    assert_eq!(missing.matches("[MISSING ASSET]").count(), 2);
    assert!(missing.contains("Files: 3 tracked"));
    assert!(missing.contains("Dependencies: 0 satisfied · 2 conflicts"));
}

#[test]
fn interface_cycles_render_finitely_and_show_changed_contract_versions() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    for (name, content) in [
        ("pipe.step", "pipe"),
        ("rod.step", "rod"),
        ("fit.txt", "20 mm"),
    ] {
        fs::write(root.join(name), content).unwrap();
    }
    ok(root, &["add", "."]);
    for asset in ["pipe.step", "rod.step"] {
        ok(
            root,
            &[
                "interface",
                "set",
                asset,
                "--name",
                "fit",
                "--file",
                "fit.txt",
            ],
        );
    }
    for (owner, target) in [("pipe.step", "rod.step"), ("rod.step", "pipe.step")] {
        ok(
            root,
            &["depend", "add", owner, "--on", target, "--interface", "fit"],
        );
    }
    ok(root, &["commit", "-m", "Mutual fit declarations"]);
    let output = human(root, &["graph"]);
    assert!(output.contains("(cycle)"));
    assert_eq!(output.matches("pipe.step").count(), 1);
    assert_eq!(output.matches("rod.step").count(), 1);
    assert_eq!(output.matches("[matches]").count(), 2);
    assert!(output.contains("requires fit@"));
    fs::write(root.join("fit.txt"), "25 mm").unwrap();
    ok(
        root,
        &[
            "interface",
            "set",
            "pipe.step",
            "--name",
            "fit",
            "--file",
            "fit.txt",
        ],
    );
    let changed = human(root, &["status"]);
    assert!(changed.contains("[INTERFACE MISMATCH]; selected interface @"));
    ok(root, &["interface", "remove", "pipe.step", "--name", "fit"]);
    assert!(human(root, &["status"]).contains("[MISSING INTERFACE]"));
}

#[test]
fn directories_contain_only_selected_versions_across_history_and_renames() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    let mechanical = root.join("mechanical");
    fs::create_dir(&mechanical).unwrap();
    fs::write(mechanical.join("pipe.step"), "pipe R1").unwrap();
    fs::write(mechanical.join("rod.step"), "rod R1").unwrap();
    ok(root, &["add", "."]);
    ok(
        root,
        &[
            "depend",
            "add",
            "mechanical/rod.step",
            "--on",
            "mechanical/pipe.step",
        ],
    );
    ok(root, &["commit", "-m", "Initial assembly"]);
    ok(root, &["tag", "original"]);
    for revision in 2..=4 {
        fs::write(mechanical.join("pipe.step"), format!("pipe R{revision}")).unwrap();
        ok(root, &["add", "mechanical/pipe.step"]);
        ok(
            root,
            &[
                "depend",
                "add",
                "mechanical/rod.step",
                "--on",
                "mechanical/pipe.step",
            ],
        );
        ok(root, &["commit", "-m", "Reviewed updated assembly"]);
    }
    ok(
        root,
        &["mv", "mechanical/pipe.step", "mechanical/tube.step"],
    );
    ok(root, &["commit", "-m", "Rename pipe"]);
    for (target, name, content) in [
        ("original", "pipe.step", "pipe R1"),
        ("main", "tube.step", "pipe R4"),
        ("original", "pipe.step", "pipe R1"),
    ] {
        ok(root, &["checkout", target]);
        let mut files: Vec<_> = fs::read_dir(&mechanical)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        let mut expected = vec!["rod.step".to_owned(), name.to_owned()];
        expected.sort();
        assert_eq!(files, expected);
        assert_eq!(fs::read_to_string(mechanical.join(name)).unwrap(), content);
        assert_eq!(
            fs::read_to_string(mechanical.join("rod.step")).unwrap(),
            "rod R1"
        );
        assert_eq!(
            ok(root, &["status"])["dependency_graph"]["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    assert!(human(root, &["status"]).contains("Detached HEAD"));
    assert_eq!(ok(root, &["log"]).as_array().unwrap().len(), 1);
    assert_eq!(
        ok(root, &["log", "--target", "main"])
            .as_array()
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn cli_versioning_and_issue_workflow() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init", "--author", "A Local Engineer"]);
    fs::write(root.join("housing.FCStd"), b"CAD\0R1\xff").unwrap();
    ok(root, &["add", "."]);
    let first = ok(root, &["commit", "-m", "Initial housing"])["commit"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(root, &["tag", "v1"]);
    ok(root, &["branch", "feature/new-housing"]);
    let issue = ok(
        root,
        &[
            "issue",
            "create",
            "--title",
            "Adjust mounting holes",
            "--body",
            "Check fit",
        ],
    );
    let id = issue["id"].as_str().unwrap();
    let head = issue["head"].as_str().unwrap();
    ok(root, &["issue", "link", id, "--commit", &first]);
    let stale = run(root, &["issue", "close", id, "--expected-head", head]);
    assert_eq!(stale.status.code(), Some(3));
    fs::write(root.join("comment.md"), "Measured **20 mm**.\n").unwrap();
    ok(root, &["issue", "comment", id, "--body-file", "comment.md"]);
    ok(root, &["issue", "link", id, "--asset", "housing.FCStd"]);
    ok(root, &["issue", "close", id]);
    assert_eq!(ok(root, &["issue", "list"]), json!([]));
    assert_eq!(
        ok(root, &["issue", "list", "--all"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fs::write(root.join("housing.FCStd"), b"CAD\0R2\xfe").unwrap();
    ok(root, &["add", "housing.FCStd"]);
    ok(root, &["commit", "-m", "Update housing"]);
    let before = fs::read(root.join("housing.FCStd")).unwrap();
    ok(root, &["checkout", "v1", "--preview"]);
    assert_eq!(fs::read(root.join("housing.FCStd")).unwrap(), before);
    ok(root, &["checkout", "v1"]);
    assert_eq!(
        fs::read(root.join("housing.FCStd")).unwrap(),
        b"CAD\0R1\xff"
    );
    assert_eq!(ok(root, &["issue", "show", id])["status"], "closed");
    ok(root, &["checkout", "main"]);
    ok(root, &["fsck"]);
    assert_eq!(ok(root, &["log"]).as_array().unwrap().len(), 2);
}

#[test]
fn json_errors_have_stable_exit_codes_and_clean_stdout() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    for args in [
        vec!["missing-command"],
        vec!["commit"],
        vec!["recover"],
        vec!["issue", "create"],
    ] {
        let output = run(root, &args);
        assert_eq!(output.status.code(), Some(2));
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["ok"], false);
        assert_eq!(error["error"]["code"], "invalid_input");
        assert!(output.stderr.is_empty());
    }
    assert_eq!(run(root, &["status"]).status.code(), Some(1));
    ok(root, &["init"]);
    assert_eq!(run(root, &["init"]).status.code(), Some(3));
    fs::write(root.join("a"), "one\n").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "One"]);
    fs::write(root.join("a"), "dirty\n").unwrap();
    assert_eq!(run(root, &["checkout", "main"]).status.code(), Some(3));
    let diff = ok(root, &["diff"]);
    assert!(diff[0]["patch"].as_str().unwrap().contains("+dirty"));
    ok(root, &["restore", "a"]);
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "one\n");
}

#[test]
fn subdirectory_paths_are_relative_to_operation_directory() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::create_dir(root.join("mechanical")).unwrap();
    fs::write(root.join("mechanical/a.step"), "cad").unwrap();
    fs::write(root.join("untracked"), "leave alone").unwrap();
    ok(&root.join("mechanical"), &["add", "."]);
    let status = ok(root, &["status"]);
    assert_eq!(status["staged"][0]["path"], "mechanical/a.step");
    assert_eq!(status["untracked"], json!(["untracked"]));
    ok(root, &["commit", "-m", "Mechanical only"]);
    ok(&root.join("mechanical"), &["mv", "a.step", "b.step"]);
    ok(root, &["commit", "-m", "Rename"]);
    assert_eq!(
        ok(&root.join("mechanical"), &["log", "b.step"])
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(run(root, &["add", "../escape"]).status.code(), Some(2));
}

#[test]
fn lock_is_respected_by_a_separate_cli_process() {
    use knot_store::FsBackend;
    let dir = tempdir().unwrap();
    let backend = FsBackend::init(dir.path(), None).unwrap();
    let output = run(dir.path(), &["status"]);
    assert_eq!(output.status.code(), Some(5));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "repository_busy");
    drop(backend);
    ok(dir.path(), &["status"]);
}

#[test]
fn dependency_commands_report_conflicts_and_explicit_wip_overrides() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::write(root.join("housing.FCStd"), "housing R5").unwrap();
    fs::write(root.join("pcb"), "PCB R3").unwrap();
    fs::write(root.join("firmware"), "firmware").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Assets"]);
    let model = ok(root, &["deps", "housing.FCStd"])["root"]["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        root,
        &[
            "depend",
            "add",
            "pcb",
            "--on",
            "housing.FCStd",
            "--revision",
            &model,
        ],
    );
    ok(root, &["depend", "add", "firmware", "--on", "pcb"]);
    let diff = ok(root, &["diff", "--staged"]);
    assert!(
        diff.as_array()
            .unwrap()
            .iter()
            .all(|d| d["metadata_changes"] == json!(["constraints"]))
    );
    ok(root, &["commit", "-m", "Declared pins"]);
    ok(root, &["tag", "valid"]);
    fs::write(root.join("housing.FCStd"), "housing R6").unwrap();
    ok(root, &["add", "housing.FCStd"]);
    let graph = ok(root, &["graph"]);
    assert_eq!(graph["requirements_satisfied"], false);
    assert_eq!(graph["declarations_only"], true);
    assert_eq!(
        ok(root, &["status"])["dependency_conflicts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let impact = ok(root, &["impact", "housing.FCStd", "--since", "HEAD"]);
    assert_eq!(impact["direct"][0]["path"], "pcb");
    assert_eq!(impact["transitive"][0]["path"], "firmware");
    let failed = run(root, &["commit", "-m", "Unsatisfied"]);
    assert_eq!(failed.status.code(), Some(3));
    let error: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(error["error"]["code"], "dependency_conflict");
    assert_eq!(
        error["error"]["details"]["conflicts"][0]["status"],
        "revision_mismatch"
    );
    assert_eq!(
        error["error"]["details"]["conflicts"][0]["requirement"]["revision"],
        model
    );
    ok(
        root,
        &["commit", "-m", "Explicit WIP", "--allow-incompatible"],
    );
    ok(root, &["checkout", "valid"]);
    assert_eq!(run(root, &["checkout", "main"]).status.code(), Some(3));
    ok(
        root,
        &["checkout", "main", "--allow-incompatible", "--preview"],
    );
    ok(root, &["checkout", "main", "--allow-incompatible"]);
    ok(root, &["fsck"]);
    assert_eq!(
        ok(root, &["graph", "--at", "valid"])["requirements_satisfied"],
        true
    );
}

#[test]
fn interface_cli_distinguishes_unchanged_and_changed_contracts() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    for (file, body) in [
        ("housing", "CAD"),
        ("pcb", "PCB"),
        ("mount.txt", "holes 20mm"),
    ] {
        fs::write(root.join(file), body).unwrap();
    }
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Assets"]);
    ok(
        root,
        &[
            "interface",
            "set",
            "housing",
            "--name",
            "pcb_mount",
            "--file",
            "mount.txt",
        ],
    );
    let interface = ok(root, &["interface", "list", "housing"]);
    let asset = interface["asset"].as_str().unwrap();
    ok(
        root,
        &[
            "depend",
            "add",
            "pcb",
            "--on",
            asset,
            "--interface",
            "pcb_mount",
        ],
    );
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Interface contract"]);
    fs::write(root.join("housing"), "CAD cosmetic change").unwrap();
    ok(root, &["add", "housing"]);
    assert_eq!(ok(root, &["graph"])["requirements_satisfied"], true);
    assert_eq!(
        ok(root, &["impact", "housing", "--since", "HEAD"])["unaffected_direct"][0]["path"],
        "pcb"
    );
    ok(root, &["commit", "-m", "Same contract"]);
    fs::write(root.join("mount.txt"), "holes 25mm").unwrap();
    ok(
        root,
        &[
            "interface",
            "set",
            "housing",
            "--name",
            "pcb_mount",
            "--file",
            "mount.txt",
        ],
    );
    assert_eq!(
        ok(root, &["deps", "pcb"])["edges"][0]["status"],
        "interface_mismatch"
    );
    assert_eq!(
        ok(root, &["deps", "housing", "--reverse", "--transitive"])["edges"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    ok(
        root,
        &[
            "depend",
            "remove",
            "pcb",
            "--on",
            "housing",
            "--interface",
            "pcb_mount",
        ],
    );
    ok(
        root,
        &["interface", "remove", "housing", "--name", "pcb_mount"],
    );
    assert!(ok(root, &["graph"])["edges"].as_array().unwrap().is_empty());
    assert_eq!(
        run(
            root,
            &[
                "depend",
                "add",
                "pcb",
                "--on",
                "housing",
                "--interface",
                "pcb_mount",
                "--revision",
                &"0".repeat(64)
            ]
        )
        .status
        .code(),
        Some(2)
    );
}

#[test]
fn dependency_paths_and_definition_files_work_from_subdirectories() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init"]);
    fs::create_dir(root.join("mechanical")).unwrap();
    fs::create_dir(root.join("electronics")).unwrap();
    fs::write(root.join("mechanical/housing"), "CAD").unwrap();
    fs::write(root.join("mechanical/mount.txt"), "holes 20mm").unwrap();
    fs::write(root.join("electronics/pcb"), "PCB").unwrap();
    ok(root, &["add", "."]);
    ok(root, &["commit", "-m", "Assets"]);
    ok(
        &root.join("mechanical"),
        &[
            "interface",
            "set",
            "housing",
            "--name",
            "mount",
            "--file",
            "mount.txt",
        ],
    );
    ok(
        &root.join("electronics"),
        &[
            "depend",
            "add",
            "pcb",
            "--on",
            "../mechanical/housing",
            "--interface",
            "mount",
        ],
    );
    let report = ok(&root.join("electronics"), &["deps", "pcb"]);
    assert_eq!(report["edges"][0]["target_path"], "mechanical/housing");
    assert_eq!(
        ok(
            &root.join("electronics"),
            &["impact", "../mechanical/housing"]
        )["direct"][0]["path"],
        "electronics/pcb"
    );
    ok(root, &["commit", "-m", "Declared interface"]);
    ok(root, &["fsck"]);
}

fn wait_for_endpoint_exit(child: &mut std::process::Child) {
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        if std::time::Instant::now() >= limit {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("endpoint failed to enforce its I/O deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn stalled_endpoint_io_releases_locks_and_rolls_back_partial_uploads() {
    use knot_core::{Backend, ObjectKind, Repository, object_header, object_id};
    use knot_store::FsBackend;
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    for mode in [
        "idle",
        "partial-control",
        "partial-upload",
        "blocked-output",
    ] {
        let dir = tempdir().unwrap();
        let backend = FsBackend::init_bare(dir.path(), None).unwrap();
        let retained = backend
            .put_object(ObjectKind::Blob, &vec![7; 1024 * 1024])
            .unwrap();
        let existing = backend.list_objects().unwrap();
        drop(backend);
        let mut command = Command::new(env!("CARGO_BIN_EXE_knot"));
        command
            .arg("-C")
            .arg(dir.path())
            .args(["serve", "--stdio", "--idle-timeout", "1"]);
        if mode == "partial-upload" {
            command.arg("--allow-writes");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let input = child.stdin.as_mut().unwrap();
        match mode {
            "idle" => {
                input.write_all(b"{\"operation\":\"advertise\"}\n").unwrap();
                input.flush().unwrap();
                let mut reply = String::new();
                output.read_line(&mut reply).unwrap();
                assert_eq!(serde_json::from_str::<Value>(&reply).unwrap()["ok"], true);
            }
            "partial-control" => {
                input.write_all(b"{\"operation\":").unwrap();
            }
            "partial-upload" => {
                let payload = b"new object rolled back after timeout";
                let id = object_id(ObjectKind::Blob, payload);
                let mut raw = object_header(ObjectKind::Blob, payload.len() as u64);
                raw.extend_from_slice(payload);
                let request = json!({"operation":"put","objects":[{"id":id,"len":raw.len()},{"id":retained,"len":100}],"updates":[]});
                writeln!(input, "{request}").unwrap();
                input.flush().unwrap();
                let mut ready = String::new();
                output.read_line(&mut ready).unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(&ready).unwrap()["data"]["ready"],
                    true
                );
                input.write_all(&raw).unwrap();
                input.write_all(b"knot\0").unwrap();
            }
            "blocked-output" => {
                writeln!(input, "{}", json!({"operation":"get","objects":[retained]})).unwrap();
                // Keep stdout open but deliberately do not drain its pipe.
            }
            _ => unreachable!(),
        }
        input.flush().unwrap();
        wait_for_endpoint_exit(&mut child);
        let repo = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
        assert_eq!(repo.backend().list_objects().unwrap(), existing, "{mode}");
        assert!(repo.backend().read_meta("incoming.json").unwrap().is_none());
        repo.fsck().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn a_stalled_ssh_peer_times_out_and_releases_the_local_lock() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let dir = tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir(&root).unwrap();
    ok(&root, &["init"]);
    ok(
        &root,
        &["remote", "add", "origin", "ssh://review@unused/peer"],
    );
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    fs::write(&ssh, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_knot"))
        .arg("-C")
        .arg(&root)
        .args(["--json", "fetch"])
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("KNOT_SYNC_IDLE_TIMEOUT_SECONDS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_endpoint_exit(&mut child);
    let result = child.wait_with_output().unwrap();
    let reply: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(reply["ok"], false);
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("progress timeout"),
        "{reply}; stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    ok(&root, &["status"]);
    ok(&root, &["fsck"]);
}

#[cfg(unix)]
#[test]
fn ssh_pipe_transport_streams_frames_and_quotes_literal_repository_paths() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    ok(&source, &["init"]);
    let content = vec![7; 128 * 1024 + 3];
    fs::write(source.join("part"), &content).unwrap();
    ok(&source, &["add", "."]);
    ok(&source, &["commit", "-m", "Initial"]);
    let peer = dir.path().join("peer'$(printf injected);");
    ok(dir.path(), &["init", peer.to_str().unwrap(), "--bare"]);
    let url = format!("ssh://review@unused{}", peer.display());
    ok(&source, &["remote", "add", "origin", &url]);
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    fs::write(
        &ssh,
        "#!/bin/sh\nfor argument do command=$argument; done\nexec /bin/sh -c \"$command\"\n",
    )
    .unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![
        bin,
        Path::new(env!("CARGO_BIN_EXE_knot"))
            .parent()
            .unwrap()
            .to_owned(),
    ];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(paths).unwrap();
    let network = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_knot"))
            .arg("-C")
            .arg(&source)
            .arg("--json")
            .args(args)
            .env("PATH", &path)
            .env("KNOT_SYNC_IDLE_TIMEOUT_SECONDS", "5")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    };
    network(&["push"]);
    let copy = dir.path().join("copy");
    network(&["clone", &url, copy.to_str().unwrap()]);
    assert_eq!(fs::read(copy.join("part")).unwrap(), content);
    ok(&copy, &["fsck"]);
}

#[test]
fn concise_status_and_validation_count_checks_and_keep_failures_visible() {
    use knot_core::{Repository, ReviewOutcome};
    use knot_store::FsBackend;
    let dir = tempdir().unwrap();
    let root = dir.path();
    let repo = Repository::open(FsBackend::init(root, None).unwrap()).unwrap();
    for name in [
        "passed-part",
        "stale-part",
        "failed-part",
        "error-part",
        "procedure.md",
    ] {
        fs::write(root.join(name), "original").unwrap();
    }
    for n in 0..64 {
        fs::write(root.join(format!("healthy-{n:03}")), "unchanged").unwrap();
    }
    repo.add(&[".".into()]).unwrap();
    repo.define_test(
        "pending-bench",
        "procedure.md",
        &["passed-part".into()],
        "Fixture check",
    )
    .unwrap();
    repo.commit("Baseline").unwrap();
    for (asset, outcome, note) in [
        (
            "passed-part",
            ReviewOutcome::Passed,
            "Measured successfully".to_owned(),
        ),
        (
            "stale-part",
            ReviewOutcome::Passed,
            "Previous measurement".to_owned(),
        ),
        ("failed-part", ReviewOutcome::Failed, "f".repeat(400)),
        (
            "error-part",
            ReviewOutcome::Error,
            "Fixture error\nretry\u{1b}[31m".to_owned(),
        ),
    ] {
        repo.record_review(
            repo.review_subject(asset, None).unwrap(),
            outcome,
            "Fixture reviewer",
            &note,
            "test fixture",
            &[],
        )
        .unwrap();
    }
    fs::write(root.join("stale-part"), "changed").unwrap();
    repo.add(&["stale-part".into()]).unwrap();
    fs::write(root.join("fresh-untracked"), "new").unwrap();
    drop(repo);
    let before = ok(root, &["status"]);
    let summary = human(root, &["status"]);
    assert!(
        summary.contains("Verification: 1 verified · 1 unverified · 1 stale · 1 failed · 1 error"),
        "{summary}"
    );
    assert!(summary.contains("1 staged · 0 unstaged · 1 untracked"));
    assert!(summary.contains("[FAILED] failed-part"));
    assert!(summary.contains("[ERROR] error-part"));
    assert!(summary.contains("Fixture error\\nretry\\u{1b}[31m"));
    assert!(!summary.contains('\u{1b}'));
    for hidden in [
        "healthy-",
        "passed-part",
        "stale-part",
        "pending-bench",
        "fresh-untracked",
    ] {
        assert!(!summary.contains(hidden), "{summary}");
    }
    assert!(!summary.contains(&"f".repeat(400)));
    assert!(summary.lines().count() < 20, "{summary}");
    let verbose = human(root, &["status", "-v"]);
    for shown in [
        "healthy-000",
        "healthy-063",
        "[verified] passed-part",
        "[STALE] stale-part",
        "[unverified] pending-bench",
        "fresh-untracked",
    ] {
        assert!(verbose.contains(shown), "missing {shown}: {verbose}");
    }
    assert!(verbose.contains(&"f".repeat(400)));
    assert_eq!(human(root, &["-v", "status"]), verbose);
    assert_eq!(ok(root, &["status", "--verbose"]), before);
    assert_eq!(ok(root, &["validate"]), before["verification"]);
    assert_eq!(ok(root, &["validate", "-v"]), before["verification"]);
    let validation = human(root, &["validate"]);
    assert!(validation.contains("[FAILED] failed-part"));
    assert!(validation.contains("[ERROR] error-part"));
    assert!(!validation.contains("passed-part"));
    assert!(!validation.contains("stale-part"));
    let validation_verbose = human(root, &["validate", "--verbose"]);
    assert!(validation_verbose.contains("[verified] passed-part"));
    assert!(validation_verbose.contains("[STALE] stale-part"));
}

#[test]
fn history_issue_lists_and_checkout_are_concise_with_full_verbose_details() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    ok(root, &["init", "--author", "Fixture author"]);
    fs::write(root.join("part"), "initial").unwrap();
    ok(root, &["add", "."]);
    ok(
        root,
        &[
            "commit",
            "-m",
            "Design summary\n\nLong engineering explanation",
        ],
    );
    let log = ok(root, &["log"]);
    let concise = human(root, &["log"]);
    assert!(concise.contains("Design summary"));
    assert!(!concise.contains("Long engineering explanation"));
    assert!(!concise.contains("Author:"));
    let verbose = human(root, &["log", "-v"]);
    assert!(verbose.contains("Author: Fixture author"));
    assert!(verbose.contains("Long engineering explanation"));
    assert_eq!(ok(root, &["log", "-v"]), log);
    let issue = ok(
        root,
        &[
            "issue",
            "create",
            "--title",
            "Check clearance",
            "--body",
            "Detailed issue body",
        ],
    );
    let id = issue["id"].as_str().unwrap();
    ok(
        root,
        &[
            "issue",
            "comment",
            id,
            "--body",
            "Detailed measurement comment",
        ],
    );
    let issues = ok(root, &["issue", "list"]);
    let concise = human(root, &["issue", "list"]);
    assert!(concise.contains("[open] Check clearance"));
    assert!(!concise.contains("Detailed issue body"));
    assert!(!concise.contains("Detailed measurement comment"));
    let verbose = human(root, &["issue", "list", "-v"]);
    assert!(verbose.contains("Detailed issue body"));
    assert!(verbose.contains("Detailed measurement comment"));
    assert_eq!(ok(root, &["issue", "list", "-v"]), issues);
    let concise = human(root, &["checkout", "HEAD"]);
    assert!(concise.contains("Working tree clean"));
    assert!(!concise.contains("part"));
    let verbose = human(root, &["checkout", "HEAD", "--verbose"]);
    assert!(verbose.contains("#1 part @"));
}
