#![allow(clippy::unwrap_used, clippy::expect_used)]
use knot_core::{Backend, Repository};
use knot_mcp::{LEGACY_VERSION, MODERN_VERSION, Server};
use knot_store::FsBackend;
use serde_json::{Value, json};
use std::{fs, io::Cursor};
use tempfile::tempdir;

fn request(server: &mut Server, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":MODERN_VERSION,"io.modelcontextprotocol/clientCapabilities":{}});
    server
        .handle(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .unwrap()
}
fn call(server: &mut Server, name: &str, args: Value) -> Value {
    request(server, "tools/call", json!({"name":name,"arguments":args}))
}
fn approved(mut args: Value, preview: &Value) -> Value {
    args["apply"] = json!(true);
    for key in [
        "expected_snapshot",
        "expected_head",
        "expected_worktree",
        "expected_refs",
    ] {
        args[key] = preview[key].clone();
    }
    args
}

#[test]
fn modern_discovery_is_stateless_and_legacy_handshake_is_supported() {
    let dir = tempdir().unwrap();
    let mut server = Server::new(dir.path(), false);
    let response = request(&mut server, "server/discover", json!({}));
    assert_eq!(response["result"]["supportedVersions"][0], MODERN_VERSION);
    assert_eq!(response["result"]["resultType"], "complete");
    let unsupported=server.handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"1900-01-01"}}})).unwrap();
    assert_eq!(unsupported["error"]["code"], -32022);
    let missing=server.handle(json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":MODERN_VERSION}}})).unwrap();
    assert_eq!(missing["error"]["code"], -32602);
    let initialized=server.handle(json!({"jsonrpc":"2.0","id":4,"method":"initialize","params":{"protocolVersion":LEGACY_VERSION,"capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).unwrap();
    assert_eq!(initialized["result"]["protocolVersion"], LEGACY_VERSION);
    assert!(
        server
            .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none()
    );
    let tools = server
        .handle(json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}))
        .unwrap();
    assert!(
        tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "knot_checkout")
    );
    assert_eq!(
        request(&mut server, "unknown", json!({}))["error"]["code"],
        -32601
    );
}

#[test]
fn agent_can_preview_write_and_commit_without_bypassing_authorization_or_stale_guards() {
    let dir = tempdir().unwrap();
    drop(FsBackend::init(dir.path(), Some("Engineer".into())).unwrap());
    let mut readonly = Server::new(dir.path(), false);
    let args = json!({"path":"notes.md","content":"A design note\n"});
    let preview =
        call(&mut readonly, "knot_write_file", args.clone())["result"]["structuredContent"].clone();
    assert_eq!(preview["preview"], true);
    assert!(!dir.path().join("notes.md").exists());
    let denied = call(
        &mut readonly,
        "knot_write_file",
        approved(args.clone(), &preview),
    );
    assert_eq!(denied["result"]["isError"], true);
    let mut writable = Server::new(dir.path(), true);
    let applied = call(&mut writable, "knot_write_file", approved(args, &preview));
    assert_eq!(applied["result"]["isError"], false);
    assert_eq!(
        fs::read_to_string(dir.path().join("notes.md")).unwrap(),
        "A design note\n"
    );
    let args = json!({"message":"Agent design note"});
    let preview =
        call(&mut writable, "knot_commit", args.clone())["result"]["structuredContent"].clone();
    let commit = call(&mut writable, "knot_commit", approved(args, &preview));
    assert_eq!(commit["result"]["isError"], false);
    let args = json!({"path":"notes.md","content":"Replacement"});
    let preview =
        call(&mut writable, "knot_write_file", args.clone())["result"]["structuredContent"].clone();
    fs::write(dir.path().join("notes.md"), "External unsaved edit").unwrap();
    let stale = call(&mut writable, "knot_write_file", approved(args, &preview));
    assert_eq!(stale["result"]["isError"], true);
    assert_eq!(
        fs::read_to_string(dir.path().join("notes.md")).unwrap(),
        "External unsaved edit"
    );
    let repo = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
    assert_eq!(repo.history("HEAD", None, 100).unwrap().len(), 1);
    repo.fsck().unwrap();
}

#[test]
fn malformed_arguments_and_path_traversal_never_mutate_state() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    let count = repo.backend().list_objects().unwrap().len();
    drop(repo);
    let mut server = Server::new(dir.path(), true);
    assert_eq!(
        call(&mut server, "knot_status", json!({"arbitrary":true}))["error"]["code"],
        -32602
    );
    assert_eq!(
        call(
            &mut server,
            "knot_write_file",
            json!({"path":5,"content":"x"})
        )["error"]["code"],
        -32602
    );
    let invalid = call(
        &mut server,
        "knot_write_file",
        json!({"path":"../outside","content":"x"}),
    );
    assert_eq!(invalid["result"]["isError"], true);
    let repo = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
    assert_eq!(repo.backend().list_objects().unwrap().len(), count);
}

#[test]
fn new_evidence_invalidates_a_preview_even_when_files_and_head_are_unchanged() {
    let dir = tempdir().unwrap();
    let repo = Repository::open(FsBackend::init(dir.path(), None).unwrap()).unwrap();
    repo.write_asset("part.md", b"part").unwrap();
    repo.commit("Part").unwrap();
    drop(repo);
    let mut server = Server::new(dir.path(), true);
    let args = json!({"path":"part.md","content":"updated part"});
    let preview =
        call(&mut server, "knot_write_file", args.clone())["result"]["structuredContent"].clone();
    let repo = Repository::open(FsBackend::open(dir.path()).unwrap()).unwrap();
    let head = repo.head_id().unwrap();
    repo.record_review(
        repo.review_subject("part.md", None).unwrap(),
        knot_core::ReviewOutcome::Passed,
        "Engineer",
        "Inspected part",
        "bench",
        &[],
    )
    .unwrap();
    assert_eq!(repo.head_id().unwrap(), head);
    drop(repo);
    let response = call(&mut server, "knot_write_file", approved(args, &preview));
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        fs::read_to_string(dir.path().join("part.md")).unwrap(),
        "part"
    );
}

#[test]
fn stdio_emits_only_framed_json_and_exits_at_eof() {
    let dir = tempdir().unwrap();
    let mut server = Server::new(dir.path(), false);
    let mut input=Cursor::new(b"not json\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\"}\n{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}\n");
    let mut out = Vec::new();
    server.serve(&mut input, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(lines[0]).unwrap()["error"]["code"],
        -32700
    );
    assert_eq!(serde_json::from_str::<Value>(lines[1]).unwrap()["id"], 8);
}
