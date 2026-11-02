use clap::{Parser, Subcommand};
mod display;

use knot_core::{
    DependencyRequest, Error, GraphReport, IssueAction, IssueLink, ObjectId, Repository, Result,
    Status,
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
    Status(Status),
    Graph(GraphReport),
}

fn value<T: serde::Serialize>(item: T) -> Result<CommandOutput> {
    Ok(CommandOutput::Data(serde_json::to_value(item)?))
}

fn run(cli: Cli) -> Result<CommandOutput> {
    if let Command::Init { path, author } = &cli.command {
        let backend = FsBackend::init(cli.directory.join(path), author.clone())?;
        return value(json!({"root": backend.root(), "branch": "main"}));
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
        Command::Add { paths: items } => {
            value(json!({"staged_snapshot": repo.add(&paths(items)?)?}))
        }
        Command::Status => Ok(CommandOutput::Status(repo.status()?)),
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
            value(repo.history(&target, filter.as_deref(), limit as usize)?)
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
        } => {
            let plan = repo.plan_checkout_with_options(&target, allow_incompatible)?;
            if preview {
                value(plan)
            } else {
                repo.apply_checkout(plan)?;
                Ok(CommandOutput::Status(repo.status()?))
            }
        }
        Command::Restore {
            paths: items,
            source,
            staged,
            preview,
        } => value(repo.restore(&paths(items)?, source.as_deref(), staged, preview)?),
        Command::Mv { from, to, preview } => {
            value(repo.move_asset(&path(&from)?, &path(&to)?, preview)?)
        }
        Command::Rm {
            path: item,
            preview,
        } => value(repo.remove(&path(&item)?, preview)?),
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
            } => {
                let request = if let Some(name) = interface {
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
