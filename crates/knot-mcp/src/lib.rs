//! Stdio MCP adapter. Authorization is local startup policy, never client identity metadata.
use knot_core::*;
use knot_store::FsBackend;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
};

pub const MODERN_VERSION: &str = "2026-07-28";
pub const LEGACY_VERSION: &str = "2025-11-25";
pub const MAX_MESSAGE: usize = 8 * 1024 * 1024;

pub struct Server {
    root: PathBuf,
    allow_writes: bool,
    initialized: bool,
    ready: bool,
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid(format!("missing string argument {name}")))
}
fn opt_str<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}
fn flag(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}
fn data<T: serde::Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

fn schema(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    write: bool,
) -> Value {
    let mut properties = properties.as_object().cloned().unwrap_or_default();
    if write {
        properties.insert("apply".into(), json!({"type":"boolean","default":false}));
        properties.insert("expected_snapshot".into(), json!({"type":"string"}));
        properties.insert("expected_head".into(), json!({"type":["string","null"]}));
        properties.insert("expected_worktree".into(), json!({"type":"string"}));
        properties.insert("expected_refs".into(), json!({"type":"string"}));
    }
    json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":!write,"destructiveHint":write,"openWorldHint":false}})
}
pub fn tools() -> Vec<Value> {
    let string = json!({"type":"string"});
    let boolean = json!({"type":"boolean"});
    let strings = json!({"type":"array","items":{"type":"string"}});
    let mut tools = vec![
        schema(
            "knot_repository_info",
            "Repository location, state, and write policy",
            json!({}),
            &[],
            false,
        ),
        schema(
            "knot_status",
            "Working changes, declared graph, and exact-input verification",
            json!({}),
            &[],
            false,
        ),
        schema(
            "knot_get_history",
            "Commit history including override record IDs",
            json!({"target":string,"asset":string}),
            &[],
            false,
        ),
        schema(
            "knot_get_asset",
            "Selected asset, revision, contracts, and outgoing requirements",
            json!({"asset":string,"at":string}),
            &["asset"],
            false,
        ),
        schema(
            "knot_get_dependencies",
            "Forward/reverse dependency traversal",
            json!({"asset":string,"at":string,"reverse":boolean,"transitive":boolean}),
            &["asset"],
            false,
        ),
        schema(
            "knot_get_impact",
            "Potential impact or comparison against committed baseline",
            json!({"asset":string,"at":string,"since":string}),
            &["asset"],
            false,
        ),
        schema(
            "knot_get_validation_results",
            "Applicable/stale reviews, tests, and upstream warnings",
            json!({"at":string}),
            &[],
            false,
        ),
        schema(
            "knot_get_graph",
            "Complete declared graph",
            json!({"at":string}),
            &[],
            false,
        ),
        schema(
            "knot_get_traceability",
            "Versioned requirements/documentation references",
            json!({"asset":string,"at":string,"reverse":boolean,"transitive":boolean}),
            &[],
            false,
        ),
        schema(
            "knot_get_issues",
            "Repository-local issues",
            json!({"all":boolean}),
            &[],
            false,
        ),
        schema(
            "knot_resolve",
            "Preview or explicitly apply a compatible configuration",
            json!({"pins":{"type":"array","items":{"type":"object","properties":{"asset":string,"revision":string},"required":["asset","revision"],"additionalProperties":false}}}),
            &[],
            true,
        ),
        schema(
            "knot_checkout",
            "Preview or explicitly apply exact branch/tag/commit checkout",
            json!({"target":string,"validated":boolean}),
            &["target"],
            true,
        ),
        schema(
            "knot_write_file",
            "Write and stage UTF-8 content through guarded core operations",
            json!({"path":string,"content":string}),
            &["path", "content"],
            true,
        ),
        schema(
            "knot_create_revision",
            "Stage files already edited in the working directory",
            json!({"paths":strings}),
            &["paths"],
            true,
        ),
        schema(
            "knot_commit",
            "Preview or commit staged changes",
            json!({"message":string}),
            &["message"],
            true,
        ),
        schema(
            "knot_add_dependency",
            "Pin an asset to a selected revision or named interface",
            json!({"asset":string,"on":string,"interface":string}),
            &["asset", "on"],
            true,
        ),
        schema(
            "knot_run_validation",
            "Record a manual test result with attributable evidence; executes no external program",
            json!({"test":string,"reviewer":string,"notes":string,"outcome":{"type":"string","enum":["passed","failed","error"]},"environment":string,"artifacts":strings}),
            &["test", "reviewer", "notes", "outcome"],
            true,
        ),
        schema(
            "knot_verify_dependency",
            "Record manual review of exact dependency revisions",
            json!({"asset":string,"on":string,"reviewer":string,"notes":string}),
            &["asset", "on", "reviewer", "notes"],
            true,
        ),
        schema(
            "knot_create_release",
            "Create immutable release after policy gates pass",
            json!({"name":string,"approver":string,"notes":string}),
            &["name", "approver"],
            true,
        ),
    ];
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    tools
}

fn validate_arguments(spec: &Value, args: &Value) -> Result<()> {
    let object = args
        .as_object()
        .ok_or_else(|| Error::Invalid("arguments must be an object".into()))?;
    for required in spec["inputSchema"]["required"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if !object.contains_key(required.as_str().unwrap_or("")) {
            return Err(Error::Invalid(format!("required argument {required}")));
        }
    }
    for (key, value) in object {
        let property = &spec["inputSchema"]["properties"][key];
        if property.is_null() {
            return Err(Error::Invalid(format!("unknown argument {key}")));
        }
        let matches = match property["type"].as_str() {
            Some("string") => value.is_string(),
            Some("boolean") => value.is_boolean(),
            Some("array") => value.is_array(),
            _ => value.is_string() || value.is_null(),
        };
        if !matches {
            return Err(Error::Invalid(format!("invalid type for {key}")));
        }
        if let Some(allowed) = property["enum"].as_array()
            && !allowed.contains(value)
        {
            return Err(Error::Invalid(format!("invalid value for {key}")));
        }
    }
    Ok(())
}

fn worktree(repo: &Repository<FsBackend>) -> Result<ObjectId> {
    let graph = repo.graph(None)?;
    let mut paths: BTreeSet<_> = repo.backend().list_files(false)?.into_iter().collect();
    paths.extend(graph.nodes.iter().map(|n| n.path.clone()));
    let mut states = Vec::new();
    for path in paths {
        let hint = graph
            .nodes
            .iter()
            .find(|n| n.path == path)
            .is_some_and(|n| n.executable);
        states.push((path.clone(), repo.backend().file_state(&path, hint)?));
    }
    Ok(object_id(ObjectKind::Blob, &canonical(&states)?))
}

fn reference_state(repo: &Repository<FsBackend>) -> Result<ObjectId> {
    let refs = repo
        .backend()
        .list_meta("refs/")?
        .into_iter()
        .map(|name| Ok((name.clone(), repo.backend().read_meta(&name)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(object_id(ObjectKind::Blob, &canonical(&refs)?))
}

impl Server {
    pub fn new(root: impl AsRef<Path>, allow_writes: bool) -> Self {
        Self {
            root: root.as_ref().to_owned(),
            allow_writes,
            initialized: false,
            ready: false,
        }
    }
    fn invoke(&self, name: &str, args: &Value, write: bool) -> Result<Value> {
        let backend = FsBackend::discover(&self.root)?;
        let root = backend.root().to_owned();
        let repo = Repository::open(backend)?;
        let apply = write && flag(args, "apply");
        let state = if write { Some(repo.status()?) } else { None };
        let fingerprint = if write { Some(worktree(&repo)?) } else { None };
        let refs = if write {
            Some(reference_state(&repo)?)
        } else {
            None
        };
        if apply {
            if !self.allow_writes {
                return Err(Error::Conflict(
                    "MCP server is read-only; operator must start it with --allow-writes".into(),
                ));
            }
            let state = state
                .as_ref()
                .ok_or_else(|| Error::Integrity("missing write preview state".into()))?;
            let expected_head: Option<ObjectId> =
                serde_json::from_value(args.get("expected_head").cloned().ok_or_else(|| {
                    Error::Invalid("expected_head (including null) is required".into())
                })?)?;
            if str_arg(args, "expected_snapshot")? != state.staged_snapshot.as_str()
                || expected_head != state.head
                || Some(str_arg(args, "expected_worktree")?)
                    != fingerprint.as_ref().map(ObjectId::as_str)
                || Some(str_arg(args, "expected_refs")?) != refs.as_ref().map(ObjectId::as_str)
            {
                return Err(Error::Conflict(
                    "preview state changed; request a new preview".into(),
                ));
            }
        }
        let preview = |details: Value| -> Result<Value> {
            let state = state
                .as_ref()
                .ok_or_else(|| Error::Integrity("missing preview".into()))?;
            Ok(
                json!({"preview":true,"operation":name,"arguments":args,"expected_snapshot":state.staged_snapshot,"expected_head":state.head,"expected_worktree":fingerprint,"expected_refs":refs,"details":details}),
            )
        };
        match name {
            "knot_repository_info" => Ok(
                json!({"root":root,"head":repo.head_id()?,"writes_enabled":self.allow_writes,"format":"filesystem/SHA-256"}),
            ),
            "knot_status" => data(repo.status()?),
            "knot_get_history" => data(repo.history(
                opt_str(args, "target").unwrap_or("HEAD"),
                opt_str(args, "asset"),
                100,
            )?),
            "knot_get_asset" => data(
                repo.dependencies(str_arg(args, "asset")?, opt_str(args, "at"), false, false)?
                    .root,
            ),
            "knot_get_dependencies" => data(repo.dependencies(
                str_arg(args, "asset")?,
                opt_str(args, "at"),
                flag(args, "reverse"),
                flag(args, "transitive"),
            )?),
            "knot_get_impact" => data(repo.impact(
                str_arg(args, "asset")?,
                opt_str(args, "at"),
                opt_str(args, "since"),
            )?),
            "knot_get_validation_results" => data(repo.verification(opt_str(args, "at"))?),
            "knot_get_graph" => data(repo.graph(opt_str(args, "at"))?),
            "knot_get_traceability" => repo.traces(
                opt_str(args, "asset"),
                opt_str(args, "at"),
                flag(args, "reverse"),
                flag(args, "transitive"),
            ),
            "knot_get_issues" => data(repo.list_issues(flag(args, "all"))?),
            "knot_resolve" => {
                let pins: Vec<ResolutionPin> =
                    serde_json::from_value(args.get("pins").cloned().unwrap_or_else(|| json!([])))?;
                let plan = repo.plan_resolution(&pins)?;
                if apply {
                    repo.apply_resolution(&plan)?;
                    data(plan)
                } else {
                    preview(data(plan)?)
                }
            }
            "knot_checkout" => {
                let target = str_arg(args, "target")?;
                if flag(args, "validated") {
                    repo.require_validated_snapshot(&repo.snapshot_at(Some(target))?)?;
                }
                let plan = repo.plan_checkout(target)?;
                if apply {
                    repo.apply_checkout(plan)?;
                    data(repo.status()?)
                } else {
                    preview(data(plan)?)
                }
            }
            "knot_write_file" => {
                let path = str_arg(args, "path")?;
                validate_path(path)?;
                if apply {
                    Ok(
                        json!({"snapshot":repo.write_asset(path,str_arg(args,"content")?.as_bytes())?}),
                    )
                } else {
                    preview(json!({"path":path,"bytes":str_arg(args,"content")?.len()}))
                }
            }
            "knot_create_revision" => {
                let paths: Vec<String> = serde_json::from_value(args["paths"].clone())?;
                for path in &paths {
                    if path != "." {
                        validate_path(path)?;
                    }
                }
                if apply {
                    Ok(json!({"snapshot":repo.add(&paths)?}))
                } else {
                    preview(data(state.as_ref())?)
                }
            }
            "knot_commit" => {
                if apply {
                    Ok(json!({"commit":repo.commit(str_arg(args,"message")?)?}))
                } else {
                    preview(data(state.as_ref())?)
                }
            }
            "knot_add_dependency" => {
                let owner = str_arg(args, "asset")?;
                let target = str_arg(args, "on")?;
                let request = if let Some(name) = opt_str(args, "interface") {
                    DependencyRequest::Interface(name.into())
                } else {
                    DependencyRequest::Revision(None)
                };
                if apply {
                    data(repo.add_dependency(owner, target, request)?)
                } else {
                    preview(
                        json!({"owner":repo.dependencies(owner,None,false,false)?.root,"target":repo.dependencies(target,None,false,false)?.root}),
                    )
                }
            }
            "knot_run_validation" => {
                let subject = repo.test_subject(str_arg(args, "test")?)?;
                let outcome = match str_arg(args, "outcome")? {
                    "passed" => ReviewOutcome::Passed,
                    "failed" => ReviewOutcome::Failed,
                    _ => ReviewOutcome::Error,
                };
                let artifacts: Vec<String> = serde_json::from_value(
                    args.get("artifacts").cloned().unwrap_or_else(|| json!([])),
                )?;
                if apply {
                    Ok(
                        json!({"record":repo.record_review(subject,outcome,str_arg(args,"reviewer")?,str_arg(args,"notes")?,opt_str(args,"environment").unwrap_or("manual agent-coordinated execution"),&artifacts)?}),
                    )
                } else {
                    preview(json!({"subject":subject,"state":repo.verification(None)?}))
                }
            }
            "knot_verify_dependency" => {
                let subject =
                    repo.review_subject(str_arg(args, "asset")?, Some(str_arg(args, "on")?))?;
                if apply {
                    Ok(
                        json!({"record":repo.record_review(subject,ReviewOutcome::Passed,str_arg(args,"reviewer")?,str_arg(args,"notes")?,"manual review recorded through MCP",&[])?}),
                    )
                } else {
                    preview(json!({"subject":subject,"state":repo.verification(None)?}))
                }
            }
            "knot_create_release" => {
                if apply {
                    Ok(
                        json!({"release":repo.create_release(str_arg(args,"name")?,str_arg(args,"approver")?,opt_str(args,"notes").unwrap_or(""))?}),
                    )
                } else {
                    preview(
                        json!({"evidence":repo.require_validated_snapshot(&repo.snapshot_at(None)?)?}),
                    )
                }
            }
            _ => Err(Error::NotFound(format!("tool {name}"))),
        }
    }

    pub fn handle(&mut self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let error = |code: i64, message: &str, details: Value| json!({"jsonrpc":"2.0","id":id.clone().unwrap_or(Value::Null),"error":{"code":code,"message":message,"data":details}});
        if request["jsonrpc"] != "2.0"
            || !request["method"].is_string()
            || id
                .as_ref()
                .is_some_and(|v| !(v.is_string() || v.is_i64() || v.is_u64()))
        {
            return Some(error(-32600, "Invalid request", json!({})));
        }
        let method = request["method"].as_str().unwrap_or("");
        if id.is_none() {
            if method == "notifications/initialized" && self.initialized {
                self.ready = true;
            }
            return None;
        }
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Some(error(-32602, "Params must be an object", json!({})));
        }
        let meta = &params["_meta"];
        let modern = !meta["io.modelcontextprotocol/protocolVersion"].is_null();
        if modern {
            if meta["io.modelcontextprotocol/protocolVersion"] != MODERN_VERSION {
                return Some(error(
                    -32022,
                    "Unsupported protocol version",
                    json!({"supported":[MODERN_VERSION,LEGACY_VERSION],"requested":meta["io.modelcontextprotocol/protocolVersion"]}),
                ));
            }
            if !meta["io.modelcontextprotocol/clientCapabilities"].is_object() {
                return Some(error(-32602, "Missing client capabilities", json!({})));
            }
        } else if method != "initialize" && !(self.ready || method == "ping") {
            return Some(error(
                -32602,
                "Modern metadata or completed legacy initialization required",
                json!({}),
            ));
        }
        let result = match method {
            "initialize" => {
                self.initialized = true;
                json!({"protocolVersion":LEGACY_VERSION,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"knot-mcp","version":env!("CARGO_PKG_VERSION")}})
            }
            "server/discover" => {
                json!({"supportedVersions":[MODERN_VERSION,LEGACY_VERSION],"serverInfo":{"name":"knot-mcp","version":env!("CARGO_PKG_VERSION")},"capabilities":{"tools":{"listChanged":false}}})
            }
            "ping" => json!({}),
            "tools/list" => {
                if params.get("cursor").is_some_and(|v| !v.is_null()) {
                    return Some(error(-32602, "Unknown cursor", json!({})));
                }
                json!({"tools":tools()})
            }
            "tools/call" => {
                let Some(spec) = tools().into_iter().find(|s| s["name"] == params["name"]) else {
                    return Some(error(-32602, "Unknown tool", json!({})));
                };
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if let Err(e) = validate_arguments(&spec, &args) {
                    return Some(error(-32602, &e.to_string(), json!({})));
                }
                match self.invoke(
                    spec["name"].as_str().unwrap_or(""),
                    &args,
                    spec["annotations"]["readOnlyHint"] == false,
                ) {
                    Ok(value) => {
                        json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":if value.is_object() {value.clone()} else {json!({"data":value})},"isError":false})
                    }
                    Err(e) => {
                        json!({"content":[{"type":"text","text":e.to_string()}],"structuredContent":{"code":e.code(),"details":e.details()},"isError":true})
                    }
                }
            }
            _ => return Some(error(-32601, "Method not found", json!({}))),
        };
        let mut result = result;
        if modern {
            result["resultType"] = json!("complete");
            result["_meta"] = json!({"io.modelcontextprotocol/serverInfo":{"name":"knot-mcp","version":env!("CARGO_PKG_VERSION")}});
        }
        Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }

    pub fn serve(
        &mut self,
        reader: &mut impl BufRead,
        writer: &mut impl Write,
    ) -> std::io::Result<()> {
        loop {
            let mut line = Vec::new();
            let count = reader
                .by_ref()
                .take(MAX_MESSAGE as u64 + 1)
                .read_until(b'\n', &mut line)?;
            if count == 0 {
                return Ok(());
            }
            if line.len() > MAX_MESSAGE {
                return Err(std::io::Error::other("MCP message exceeds 8 MiB"));
            }
            let response = match serde_json::from_slice::<Value>(&line) {
                Ok(request) => self.handle(request),
                Err(_) => Some(
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
                ),
            };
            if let Some(response) = response {
                serde_json::to_writer(&mut *writer, &response)?;
                writeln!(writer)?;
                writer.flush()?;
            }
        }
    }
}
