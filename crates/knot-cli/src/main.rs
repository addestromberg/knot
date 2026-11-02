use clap::{Parser, Subcommand};
mod display;

use knot_core::{
    Backend, DependencyRequest, Error, GraphReport, IssueAction, IssueLink, ObjectId, Repository,
    ResolutionPin, Result, ReviewOutcome, Status,
};
use knot_store::FsBackend;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    name = "knot",
    version,
    about = "Offline engineering version control and local issues"
)]
struct Cli {
    /// Emit a versioned JSON envelope.
    #[arg(long, global = true)]
    json: bool,
    /// Operate from this directory.
    #[arg(short = 'C', long, global = true, default_value = ".")]
    directory: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a repository without overwriting existing metadata.
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        author: Option<String>,
        /// Create a peer repository without a working tree.
        #[arg(long)]
        bare: bool,
    },
    Clone {
        url: String,
        path: PathBuf,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        allow_incompatible: bool,
    },
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
    Fetch {
        #[arg(default_value = "origin")]
        remote: String,
    },
    Push {
        #[arg(default_value = "origin")]
        remote: String,
        branch: Option<String>,
    },
    Pull {
        #[arg(default_value = "origin")]
        remote: String,
        #[arg(long)]
        allow_incompatible: bool,
    },
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },
    /// Object/ref protocol endpoint for an authenticated SSH session.
    Serve {
        #[arg(long, required = true)]
        stdio: bool,
        #[arg(long)]
        allow_writes: bool,
    },
    /// Stage files, modifications, and tracked deletions in the supplied scopes.
    Add {
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Show working changes and the staged dependency graph.
    Status,
    Commit {
        #[arg(short, long)]
        message: String,
        /// Explicitly record a work-in-progress snapshot with unsatisfied requirements.
        #[arg(long)]
        allow_incompatible: bool,
    },
    Log {
        #[arg(long, default_value = "HEAD")]
        target: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..))]
        limit: u32,
        path: Option<String>,
    },
    Diff {
        #[arg(long)]
        staged: bool,
    },
    /// List branches, or create one at an existing commit.
    Branch {
        name: Option<String>,
        #[arg(default_value = "HEAD")]
        target: String,
    },
    /// List tags, or create an immutable tag.
    Tag {
        name: Option<String>,
        #[arg(default_value = "HEAD")]
        target: String,
    },
    Checkout {
        target: String,
        #[arg(long)]
        preview: bool,
        /// Restore an explicitly recorded incompatible snapshot.
        #[arg(long)]
        allow_incompatible: bool,
        /// Resolve asset@full-revision; preview unless --apply is supplied.
        #[arg(long, conflicts_with = "allow_incompatible")]
        resolve: bool,
        #[arg(long, requires = "resolve", conflicts_with = "preview")]
        apply: bool,
        #[arg(long, conflicts_with_all = ["resolve", "allow_incompatible"])]
        validated: bool,
    },
    /// Preview compatible revisions; explicitly apply to stage the resolved checkout.
    Resolve {
        asset: Option<String>,
        #[arg(long)]
        pin: Vec<String>,
        #[arg(long)]
        apply: bool,
    },
    /// Record a manual review for an asset or dependency at the staged revisions.
    Verify {
        asset: String,
        #[arg(long)]
        on: Option<String>,
        #[arg(long)]
        reviewer: String,
        #[arg(long)]
        notes: String,
        #[arg(long, default_value = "passed", value_parser = ["passed", "failed", "error"])]
        outcome: String,
        #[arg(long, default_value = "manual inspection")]
        environment: String,
        #[arg(long)]
        artifact: Vec<String>,
    },
    /// Override dependency reviews and rebind accepted requirements; records an audit trail.
    Rape {
        /// Accept every per-dependency guard without prompting.
        #[arg(long)]
        forced: bool,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long, default_value = "Dependency verification overridden by operator")]
        notes: String,
    },
    /// Inspect manual test state, or record a result with --record.
    Validate {
        test: Option<String>,
        #[arg(long)]
        affected: bool,
        #[arg(long, requires_all = ["test", "reviewer", "notes"], value_parser = ["passed", "failed", "error"])]
        record: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long, default_value = "manual execution")]
        environment: String,
        #[arg(long)]
        artifact: Vec<String>,
    },
    Tests {
        #[command(subcommand)]
        command: TestCommand,
    },
    /// Query versioned traceability links, or add one using --to and --relation.
    Trace {
        asset: Option<String>,
        #[arg(long, requires = "asset", conflicts_with = "at")]
        to: Option<String>,
        #[arg(long, default_value = "documents", value_parser = ["documents", "satisfies", "verifies", "describes"])]
        relation: String,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        reverse: bool,
        #[arg(long)]
        transitive: bool,
    },
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    /// Restore tracked files from staging, or unstage using --staged.
    Restore {
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        staged: bool,
        #[arg(long)]
        preview: bool,
    },
    /// Rename a tracked file while preserving its stable identity.
    Mv {
        from: String,
        to: String,
        #[arg(long)]
        preview: bool,
    },
    Rm {
        path: String,
        #[arg(long)]
        preview: bool,
    },
    /// Verify all objects, references, and snapshot invariants.
    Fsck,
    /// Finish or roll back an interrupted transaction.
    Recover {
        #[arg(long, conflicts_with = "abort", required_unless_present = "abort")]
        r#continue: bool,
        #[arg(long, conflicts_with = "continue")]
        abort: bool,
    },
    /// Stage dependency declarations on stable asset identities.
    Depend {
        #[command(subcommand)]
        command: DependCommand,
    },
    /// Query outgoing or reverse dependencies in staging or a committed snapshot.
    Deps {
        asset: String,
        #[arg(long)]
        reverse: bool,
        #[arg(long)]
        transitive: bool,
        #[arg(long)]
        at: Option<String>,
    },
    /// Query potential impact, or compare declared contracts with --since.
    Impact {
        asset: String,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        since: Option<String>,
    },
    /// Inspect the complete declared graph and unsatisfied requirements.
    Graph {
        #[arg(long)]
        at: Option<String>,
    },
    /// Manage versioned named contracts exported by an asset revision.
    Interface {
        #[command(subcommand)]
        command: InterfaceCommand,
    },
    /// Manage issues with history independent of engineering checkouts.
    Issue {
        #[command(subcommand)]
        command: IssueCommand,
    },
}
#[derive(Subcommand)]
enum RemoteCommand {
    Add { name: String, url: String },
    List,
    Remove { name: String },
    SetUrl { name: String, url: String },
}
#[derive(Subcommand)]
enum PeerCommand {
    /// Listen for LAN-only announcements, or list optional mesh peers.
    Discover {
        #[arg(long,default_value_t=5,value_parser=clap::value_parser!(u64).range(1..=60))]
        seconds: u64,
        #[arg(long)]
        mesh: bool,
    },
    /// Opt in to advertising a repository connection hint on the LAN.
    Announce {
        #[arg(long)]
        name: String,
        #[arg(long)]
        url: String,
        /// Zero announces until interrupted.
        #[arg(long, default_value_t = 0)]
        seconds: u64,
    },
    /// Save a peer URL; the transport still authenticates access on connection.
    Trust {
        name: String,
        url: String,
    },
    List,
    /// Start the optional Tailscale WireGuard mesh and perform provider authorization.
    MeshUp {
        #[arg(long)]
        ssh: bool,
        #[arg(long)]
        login_server: Option<String>,
    },
    /// Save a Tailscale SSH peer with an absolute repository path.
    Connect {
        name: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        user: String,
        #[arg(long)]
        repository: String,
    },
}
#[derive(Subcommand)]
enum TestCommand {
    Define {
        name: String,
        #[arg(long)]
        procedure: String,
        #[arg(long, required = true)]
        input: Vec<String>,
        #[arg(long, default_value = "")]
        description: String,
    },
    List {
        #[arg(long)]
        at: Option<String>,
    },
    History {
        name: String,
    },
}
#[derive(Subcommand)]
enum ReleaseCommand {
    Create {
        name: String,
        #[arg(long)]
        approver: String,
        #[arg(long, default_value = "")]
        notes: String,
    },
    List,
    Show {
        name: String,
    },
}
#[derive(Subcommand)]
enum DependCommand {
    Add {
        asset: String,
        #[arg(long)]
        on: String,
        /// Exact revision hash; defaults to the target's staged revision.
        #[arg(long, conflicts_with = "interface")]
        revision: Option<ObjectId>,
        /// Require the target's current version of this named interface.
        #[arg(long)]
        interface: Option<String>,
        #[arg(long, conflicts_with_all = ["revision", "interface"])]
        min_revision: Option<ObjectId>,
        #[arg(long, requires = "min_revision")]
        max_revision: Option<ObjectId>,
    },
    Remove {
        asset: String,
        #[arg(long)]
        on: String,
        #[arg(long)]
        interface: Option<String>,
    },
}
#[derive(Subcommand)]
enum InterfaceCommand {
    Set {
        asset: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        file: String,
    },
    Remove {
        asset: String,
        #[arg(long)]
        name: String,
    },
    List {
        asset: String,
        #[arg(long)]
        at: Option<String>,
    },
}

#[derive(Subcommand)]
enum IssueCommand {
    Create {
        #[arg(long)]
        title: String,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
    },
    List {
        #[arg(long)]
        all: bool,
    },
    Show {
        id: String,
    },
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long)]
        expected_head: Option<ObjectId>,
    },
    Comment {
        id: String,
        #[arg(
            long,
            conflicts_with = "body_file",
            required_unless_present = "body_file"
        )]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long)]
        expected_head: Option<ObjectId>,
    },
    Close {
        id: String,
        #[arg(long)]
        expected_head: Option<ObjectId>,
    },
    Reopen {
        id: String,
        #[arg(long)]
        expected_head: Option<ObjectId>,
    },
    Link {
        id: String,
        #[arg(long, conflicts_with = "asset", required_unless_present = "asset")]
        commit: Option<String>,
        /// Stable asset object ID, or a tracked path.
        #[arg(long)]
        asset: Option<String>,
        #[arg(long)]
        expected_head: Option<ObjectId>,
    },
}

fn body_value(base: &Path, body: Option<String>, file: Option<PathBuf>) -> Result<Option<String>> {
    if let Some(file) = file {
        Ok(Some(std::fs::read_to_string(base.join(file))?))
    } else {
        Ok(body)
    }
}

fn relative_path(base: &Path, root: &Path, input: &str) -> Result<String> {
    let mut normalized = PathBuf::new();
    for component in base.join(input).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(Error::Invalid("path escapes repository".into()));
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let relative = normalized
        .strip_prefix(root)
        .map_err(|_| Error::Invalid("path escapes repository".into()))?;
    let components: Result<Vec<_>> = relative
        .iter()
        .map(|s| {
            s.to_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::Invalid("path is not UTF-8".into()))
        })
        .collect();
    let result = components?.join("/");
    if result.is_empty() {
        Ok(".".into())
    } else {
        knot_core::validate_path(&result)?;
        Ok(result)
    }
}
#[derive(serde::Serialize)]
#[serde(untagged)]
enum CommandOutput {
    Data(Value),
    Status(Box<Status>),
    Graph(GraphReport),
    History(Vec<knot_core::HistoryEntry>),
}

fn value<T: serde::Serialize>(item: T) -> Result<CommandOutput> {
    Ok(CommandOutput::Data(serde_json::to_value(item)?))
}

fn outcome(value: &str) -> Result<ReviewOutcome> {
    match value {
        "passed" => Ok(ReviewOutcome::Passed),
        "failed" => Ok(ReviewOutcome::Failed),
        "error" => Ok(ReviewOutcome::Error),
        _ => Err(Error::Invalid("unknown review outcome".into())),
    }
}

fn normalize_url(base: &Path, url: &str) -> Result<String> {
    knot_store::normalize_remote_url(base, url)
}

fn run(cli: Cli) -> Result<CommandOutput> {
    if let Command::Init { path, author, bare } = &cli.command {
        let backend = if *bare {
            FsBackend::init_bare(cli.directory.join(path), author.clone())?
        } else {
            FsBackend::init(cli.directory.join(path), author.clone())?
        };
        return value(json!({"root": backend.root(), "branch": "main","bare":bare}));
    }
    if let Command::Clone {
        url,
        path,
        branch,
        allow_incompatible,
    } = &cli.command
    {
        let url = normalize_url(&cli.directory, url)?;
        return value(knot_store::clone_repository(
            &url,
            &cli.directory.join(path),
            branch.as_deref(),
            *allow_incompatible,
        )?);
    }
    if let Command::Peer { command } = &cli.command {
        match command {
            PeerCommand::Discover { seconds, mesh } => {
                return if *mesh {
                    value(knot_store::mesh_peers()?)
                } else {
                    value(knot_store::discover_peers(std::time::Duration::from_secs(
                        *seconds,
                    ))?)
                };
            }
            PeerCommand::Announce { name, url, seconds } => {
                knot_store::announce_peer(name, url, std::time::Duration::from_secs(*seconds))?;
                return value(json!({"announced":name}));
            }
            PeerCommand::MeshUp { ssh, login_server } => {
                knot_store::mesh_up(*ssh, login_server.as_deref())?;
                return value(json!({"mesh":"tailscale","ready":true}));
            }
            _ => {}
        }
    }
    let base = std::fs::canonicalize(&cli.directory)?;
    let backend = FsBackend::discover(&base)?;
    let root = backend.root().to_owned();
    let path = |s: &str| relative_path(&base, &root, s);
    let paths =
        |items: Vec<String>| -> Result<Vec<String>> { items.iter().map(|s| path(s)).collect() };
    let selector = |s: &str| -> Result<String> {
        if s.parse::<ObjectId>().is_ok() {
            Ok(s.to_owned())
        } else {
            path(s)
        }
    };
    let repo = Repository::open(backend)?;
    match cli.command {
        Command::Init { .. } => Err(Error::Invalid("unexpected init operation".into())),
        Command::Clone { .. } | Command::Serve { .. } => {
            Err(Error::Invalid("unexpected standalone command".into()))
        }
        Command::Remote { command } => match command {
            RemoteCommand::Add { name, url } => {
                knot_store::remote_add(&repo, &name, &normalize_url(&base, &url)?)?;
                value(knot_store::remote_list(&repo)?)
            }
            RemoteCommand::Remove { name } => {
                knot_store::remote_remove(&repo, &name)?;
                value(knot_store::remote_list(&repo)?)
            }
            RemoteCommand::SetUrl { name, url } => {
                knot_store::remote_set_url(&repo, &name, &normalize_url(&base, &url)?)?;
                value(knot_store::remote_list(&repo)?)
            }
            RemoteCommand::List => value(knot_store::remote_list(&repo)?),
        },
        Command::Fetch { remote } => value(knot_store::fetch(&repo, &remote)?),
        Command::Push { remote, branch } => {
            value(knot_store::push(&repo, &remote, branch.as_deref())?)
        }
        Command::Pull {
            remote,
            allow_incompatible,
        } => value(knot_store::pull(&repo, &remote, allow_incompatible)?),
        Command::Peer { command } => match command {
            PeerCommand::Trust { name, url } => {
                let url = normalize_url(&base, &url)?;
                if matches!(
                    knot_store::RemoteAddress::parse(&url)?,
                    knot_store::RemoteAddress::Local(_)
                ) {
                    return Err(Error::Invalid(
                        "peer trust requires an SSH or Tailscale URL".into(),
                    ));
                }
                knot_store::remote_add(&repo, &name, &url)?;
                value(knot_store::remote_list(&repo)?)
            }
            PeerCommand::Connect {
                name,
                host,
                user,
                repository,
            } => {
                if !repository.starts_with('/') {
                    return Err(Error::Invalid(
                        "peer repository path must be absolute".into(),
                    ));
                }
                let url = format!("tailscale://{user}@{host}{repository}");
                knot_store::remote_add(&repo, &name, &url)?;
                value(knot_store::remote_list(&repo)?)
            }
            PeerCommand::List => value(knot_store::remote_list(&repo)?),
            _ => Err(Error::Invalid("unexpected standalone peer command".into())),
        },
        Command::Add { paths: items } => {
            value(json!({"staged_snapshot": repo.add(&paths(items)?)?}))
        }
        Command::Status => Ok(CommandOutput::Status(Box::new(repo.status()?))),
        Command::Commit {
            message,
            allow_incompatible,
        } => value(json!({"commit": repo.commit_with_options(&message, allow_incompatible)?})),
        Command::Log {
            target,
            path: item,
            limit,
        } => {
            let filter = item.map(|p| path(&p)).transpose()?;
            Ok(CommandOutput::History(repo.history(
                &target,
                filter.as_deref(),
                limit as usize,
            )?))
        }
        Command::Diff { staged } => value(repo.diff(staged)?),
        Command::Branch { name, target } => {
            if let Some(name) = name {
                value(json!({"branch": name, "commit": repo.create_ref(&name, &target, false)?}))
            } else {
                value(repo.refs(false)?)
            }
        }
        Command::Tag { name, target } => {
            if let Some(name) = name {
                value(json!({"tag": name, "commit": repo.create_ref(&name, &target, true)?}))
            } else {
                value(repo.refs(true)?)
            }
        }
        Command::Checkout {
            target,
            preview,
            allow_incompatible,
            resolve,
            apply,
            validated,
        } => {
            if resolve {
                let (asset, revision) = target.rsplit_once('@').ok_or_else(|| {
                    Error::Invalid("use asset@full-revision with --resolve".into())
                })?;
                let plan = repo.plan_resolution(&[ResolutionPin {
                    asset: selector(asset)?,
                    revision: revision.parse()?,
                }])?;
                if apply {
                    repo.apply_resolution(&plan)?;
                }
                return value(plan);
            }
            if validated {
                repo.require_validated_snapshot(&repo.snapshot_at(Some(&target))?)?;
            }
            let plan = repo.plan_checkout_with_options(&target, allow_incompatible)?;
            if preview {
                value(plan)
            } else {
                repo.apply_checkout(plan)?;
                Ok(CommandOutput::Status(Box::new(repo.status()?)))
            }
        }
        Command::Restore {
            paths: items,
            source,
            staged,
            preview,
        } => value(repo.restore(&paths(items)?, source.as_deref(), staged, preview)?),
        Command::Resolve {
            asset,
            mut pin,
            apply,
        } => {
            pin.extend(asset);
            let pins = pin
                .into_iter()
                .map(|pin| {
                    let (asset, revision) = pin
                        .rsplit_once('@')
                        .ok_or_else(|| Error::Invalid("pins use asset@full-revision".into()))?;
                    Ok(ResolutionPin {
                        asset: selector(asset)?,
                        revision: revision.parse()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let plan = repo.plan_resolution(&pins)?;
            if apply {
                repo.apply_resolution(&plan)?;
            }
            value(plan)
        }
        Command::Mv { from, to, preview } => {
            value(repo.move_asset(&path(&from)?, &path(&to)?, preview)?)
        }
        Command::Rm {
            path: item,
            preview,
        } => value(repo.remove(&path(&item)?, preview)?),
        Command::Rape {
            forced,
            reviewer,
            notes,
        } => {
            if cli.json && !forced {
                return Err(Error::Invalid(
                    "interactive guards require human output; use --forced for JSON automation"
                        .into(),
                ));
            }
            let report = repo.verification(None)?;
            let mut accepted = Vec::new();
            for entry in report.dependencies {
                let yes = if forced {
                    true
                } else {
                    eprint!(
                        "Override {} -> {} and reset its requirements to selected revisions? [y/N] ",
                        entry.path,
                        entry.target_path.as_deref().unwrap_or("missing asset")
                    );
                    std::io::stderr().flush()?;
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer)?;
                    ["y", "yes"].contains(&answer.trim().to_ascii_lowercase().as_str())
                };
                if yes {
                    accepted.push(entry.subject);
                }
            }
            let reviewer = reviewer
                .or(repo.backend().author()?)
                .unwrap_or_else(|| "local operator".into());
            value(
                json!({"overrides":repo.override_dependency_reviews(&accepted,&reviewer,&notes)?,"accepted":accepted.len(),"commit_marker":"(raped)"}),
            )
        }
        Command::Verify {
            asset,
            on,
            reviewer,
            notes,
            outcome: result,
            environment,
            artifact,
        } => {
            let on = on.map(|s| selector(&s)).transpose()?;
            let subject = repo.review_subject(&selector(&asset)?, on.as_deref())?;
            value(
                json!({"record":repo.record_review(subject,outcome(&result)?,&reviewer,&notes,&environment,&paths(artifact)?)?}),
            )
        }
        Command::Validate {
            test,
            affected,
            record,
            reviewer,
            notes,
            environment,
            artifact,
        } => {
            if let Some(result) = record {
                let subject = repo.test_subject(
                    test.as_deref()
                        .ok_or_else(|| Error::Invalid("test is required".into()))?,
                )?;
                value(
                    json!({"record":repo.record_review(subject,outcome(&result)?,reviewer.as_deref().ok_or_else(||Error::Invalid("reviewer required".into()))?,notes.as_deref().ok_or_else(||Error::Invalid("notes required".into()))?,&environment,&paths(artifact)?)?}),
                )
            } else {
                let mut report = repo.verification(None)?;
                if let Some(name) = test {
                    report.tests.retain(|t| t.path == name);
                }
                if affected {
                    report
                        .tests
                        .retain(|t| t.state != knot_core::VerificationState::Verified);
                }
                value(report)
            }
        }
        Command::Tests { command } => match command {
            TestCommand::Define {
                name,
                procedure,
                input,
                description,
            } => value(
                json!({"definition":repo.define_test(&name,&path(&procedure)?,&paths(input)?,&description)?}),
            ),
            TestCommand::List { at } => {
                value(repo.tests_in_snapshot(&repo.snapshot_at(at.as_deref())?)?)
            }
            TestCommand::History { name } => {
                value(repo.review_history(&repo.test_subject(&name)?)?)
            }
        },
        Command::Trace {
            asset,
            to,
            relation,
            at,
            reverse,
            transitive,
        } => {
            let asset = asset.map(|s| selector(&s)).transpose()?;
            if let Some(to) = to {
                value(
                    json!({"link":repo.add_trace(asset.as_deref().ok_or_else(||Error::Invalid("source required".into()))?,&selector(&to)?,&relation)?}),
                )
            } else {
                value(repo.traces(asset.as_deref(), at.as_deref(), reverse, transitive)?)
            }
        }
        Command::Release { command } => match command {
            ReleaseCommand::Create {
                name,
                approver,
                notes,
            } => value(json!({"release":repo.create_release(&name,&approver,&notes)?})),
            ReleaseCommand::List => value(repo.releases()?),
            ReleaseCommand::Show { name } => value(
                repo.releases()?
                    .into_iter()
                    .find(|(_, r)| r.name == name)
                    .ok_or_else(|| Error::NotFound(format!("release {name}")))?,
            ),
        },
        Command::Fsck => value(repo.fsck()?),
        Command::Recover { abort, .. } => {
            repo.recover(abort)?;
            value(json!({"recovered": true, "aborted": abort}))
        }
        Command::Graph { at } => Ok(CommandOutput::Graph(repo.graph(at.as_deref())?)),
        Command::Deps {
            asset,
            reverse,
            transitive,
            at,
        } => value(repo.dependencies(&selector(&asset)?, at.as_deref(), reverse, transitive)?),
        Command::Impact { asset, at, since } => {
            value(repo.impact(&selector(&asset)?, at.as_deref(), since.as_deref())?)
        }
        Command::Depend { command } => match command {
            DependCommand::Add {
                asset,
                on,
                revision,
                interface,
                min_revision,
                max_revision,
            } => {
                let request = if let Some(minimum) = min_revision {
                    DependencyRequest::RevisionRange {
                        minimum,
                        maximum: max_revision,
                    }
                } else if let Some(name) = interface {
                    DependencyRequest::Interface(name)
                } else {
                    DependencyRequest::Revision(revision)
                };
                value(repo.add_dependency(&selector(&asset)?, &selector(&on)?, request)?)
            }
            DependCommand::Remove {
                asset,
                on,
                interface,
            } => value(repo.remove_dependency(
                &selector(&asset)?,
                &selector(&on)?,
                interface.as_deref(),
            )?),
        },
        Command::Interface { command } => match command {
            InterfaceCommand::Set { asset, name, file } => {
                value(repo.set_interface(&selector(&asset)?, &name, &path(&file)?)?)
            }
            InterfaceCommand::Remove { asset, name } => {
                value(repo.remove_interface(&selector(&asset)?, &name)?)
            }
            InterfaceCommand::List { asset, at } => value(
                repo.dependencies(&selector(&asset)?, at.as_deref(), false, false)?
                    .root,
            ),
        },
        Command::Issue { command } => match command {
            IssueCommand::Create {
                title,
                body,
                body_file,
            } => value(repo.create_issue(
                &title,
                &body_value(&base, body, body_file)?.unwrap_or_default(),
            )?),
            IssueCommand::List { all } => value(repo.list_issues(all)?),
            IssueCommand::Show { id } => value(repo.issue(&id)?),
            IssueCommand::Edit {
                id,
                title,
                body,
                body_file,
                expected_head,
            } => value(repo.update_issue(
                &id,
                IssueAction::Edit {
                    title,
                    body: body_value(&base, body, body_file)?,
                },
                expected_head.as_ref(),
            )?),
            IssueCommand::Comment {
                id,
                body,
                body_file,
                expected_head,
            } => value(repo.update_issue(
                &id,
                IssueAction::Comment {
                    body: body_value(&base, body, body_file)?.unwrap_or_default(),
                },
                expected_head.as_ref(),
            )?),
            IssueCommand::Close { id, expected_head } => {
                value(repo.update_issue(&id, IssueAction::Close, expected_head.as_ref())?)
            }
            IssueCommand::Reopen { id, expected_head } => {
                value(repo.update_issue(&id, IssueAction::Reopen, expected_head.as_ref())?)
            }
            IssueCommand::Link {
                id,
                commit,
                asset,
                expected_head,
            } => {
                let target = if let Some(commit) = commit {
                    IssueLink::Commit(repo.resolve(&commit)?)
                } else if let Some(asset) = asset {
                    IssueLink::Asset(if let Ok(id) = asset.parse::<ObjectId>() {
                        id
                    } else {
                        repo.asset_at(&path(&asset)?)?
                    })
                } else {
                    return Err(Error::Invalid("provide --commit or --asset".into()));
                };
                value(repo.update_issue(
                    &id,
                    IssueAction::Link { target },
                    expected_head.as_ref(),
                )?)
            }
        },
    }
}

fn output(value: &Value, json_output: bool) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if json_output {
        serde_json::to_writer(&mut out, value)?;
    } else {
        serde_json::to_writer_pretty(&mut out, value)?;
    }
    writeln!(out)
}

fn success_output(data: CommandOutput, json_output: bool) -> std::io::Result<()> {
    if json_output {
        return output(&json!({"schema_version":1,"ok":true,"data":data}), true);
    }
    let stdout = std::io::stdout();
    match data {
        CommandOutput::Status(status) => display::status(&mut stdout.lock(), &status),
        CommandOutput::Graph(graph) => display::graph(&mut stdout.lock(), &graph),
        CommandOutput::History(history) => display::history(&mut stdout.lock(), &history),
        CommandOutput::Data(data) => output(&data, false),
    }
}
fn main() -> ExitCode {
    let wants_json = std::env::args_os().any(|a| a == "--json");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if !error.use_stderr() {
                let _ = error.print();
                return ExitCode::SUCCESS;
            }
            if wants_json {
                let result = json!({"schema_version":1,"ok":false,"error":{"code":"invalid_input","message":error.to_string(),"details":{}}});
                let _ = output(&result, true);
            } else {
                let _ = error.print();
            }
            return ExitCode::from(2);
        }
    };
    if let Command::Serve { allow_writes, .. } = &cli.command {
        let result = (|| -> Result<()> {
            let repo = Repository::open(FsBackend::discover(&cli.directory)?)?;
            knot_store::serve_repository(
                &repo,
                &mut std::io::stdin().lock(),
                &mut std::io::stdout().lock(),
                *allow_writes,
            )
        })();
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(error.exit_code())
            }
        };
    }
    let json_output = cli.json;
    match run(cli) {
        Ok(data) => match success_output(data, json_output) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            if json_output {
                let result = json!({"schema_version":1,"ok":false,"error":{"code":error.code(),"message":error.to_string(),"details":error.details()}});
                let _ = output(&result, true);
            } else {
                eprintln!("{error}");
            }
            ExitCode::from(error.exit_code())
        }
    }
}
