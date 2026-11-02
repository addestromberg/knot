//! Verified object/ref exchange over local paths or an authenticated OpenSSH subprocess.
use crate::{Error, FsBackend, Result};
use knot_core::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

const MAX_CONTROL: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub version: u32,
    pub refs: BTreeMap<String, ObjectId>,
    pub objects: Vec<ObjectId>,
    pub head: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectFrame {
    pub id: ObjectId,
    pub len: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Advertise,
    Get {
        objects: Vec<ObjectId>,
    },
    Put {
        objects: Vec<ObjectFrame>,
        updates: Vec<ReferenceUpdate>,
    },
}
#[derive(Clone, Debug, Serialize)]
pub struct TransferReport {
    pub objects: usize,
    pub references: usize,
    pub remote: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteAddress {
    Local(PathBuf),
    Ssh {
        host: String,
        port: Option<u16>,
        path: String,
    },
    Mesh {
        host: String,
        path: String,
    },
}
impl RemoteAddress {
    pub fn parse(url: &str) -> Result<Self> {
        if let Some(tail) = url.strip_prefix("tailscale://") {
            return match Self::parse(&format!("ssh://{tail}"))? {
                Self::Ssh {
                    host,
                    port: None,
                    path,
                } => Ok(Self::Mesh { host, path }),
                _ => Err(Error::Invalid(
                    "Tailscale SSH uses the provider's SSH port; omit an explicit port".into(),
                )),
            };
        }
        if let Some(tail) = url.strip_prefix("ssh://") {
            let (authority, path) = tail.split_once('/').ok_or_else(|| {
                Error::Invalid("SSH URL requires an absolute repository path".into())
            })?;
            let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
                (
                    host,
                    Some(
                        port.parse::<u16>()
                            .map_err(|_| Error::Invalid("invalid SSH port".into()))?,
                    ),
                )
            } else {
                (authority, None)
            };
            if host.is_empty()
                || host.starts_with('-')
                || host.split('@').count() > 2
                || !host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b))
                || host.split('@').any(|p| p.is_empty() || p.starts_with('-'))
                || port == Some(0)
                || path.is_empty()
                || path.chars().any(char::is_control)
            {
                return Err(Error::Invalid("invalid SSH host, port, or path".into()));
            }
            Ok(Self::Ssh {
                host: host.into(),
                port,
                path: format!("/{path}"),
            })
        } else {
            if url.contains("://") && !url.starts_with("file://") {
                return Err(Error::Invalid(
                    "supported remotes are local paths, file://, and ssh://".into(),
                ));
            }
            let path = url.strip_prefix("file://").unwrap_or(url);
            if path.is_empty() {
                return Err(Error::Invalid("remote path is empty".into()));
            }
            Ok(Self::Local(PathBuf::from(path)))
        }
    }
}

pub fn normalize_remote_url(base: &Path, url: &str) -> Result<String> {
    match RemoteAddress::parse(url)? {
        RemoteAddress::Local(path) => Ok(fs::canonicalize(base.join(path))?
            .to_str()
            .ok_or_else(|| Error::Invalid("remote path is not UTF-8".into()))?
            .into()),
        _ => Ok(url.into()),
    }
}

pub fn remote_add(repo: &Repository<FsBackend>, name: &str, url: &str) -> Result<()> {
    check_remote_name(name)?;
    ensure_ready(repo)?;
    let url = normalize_remote_url(repo.backend().root(), url)?;
    let path = format!("remotes/{name}");
    if repo.backend().read_meta(&path)?.is_some() {
        return Err(Error::Conflict("remote already exists".into()));
    }
    let remote = Remote { url };
    let bytes = canonical(&remote)?;
    repo.backend().write_meta(
        &path,
        Some(std::str::from_utf8(&bytes).map_err(|e| Error::Integrity(e.to_string()))?),
    )
}
pub fn remote_list(repo: &Repository<FsBackend>) -> Result<BTreeMap<String, Remote>> {
    repo.backend()
        .list_meta("remotes/")?
        .into_iter()
        .map(|path| {
            let text = repo
                .backend()
                .read_meta(&path)?
                .ok_or_else(|| Error::Integrity("missing remote config".into()))?;
            Ok((
                path["remotes/".len()..].into(),
                serde_json::from_str(&text)?,
            ))
        })
        .collect()
}
pub fn remote_remove(repo: &Repository<FsBackend>, name: &str) -> Result<()> {
    ensure_ready(repo)?;
    check_remote_name(name)?;
    if repo
        .backend()
        .read_meta(&format!("remotes/{name}"))?
        .is_none()
    {
        return Err(Error::NotFound(format!("remote {name}")));
    }
    repo.backend().write_meta(&format!("remotes/{name}"), None)
}

pub fn remote_set_url(repo: &Repository<FsBackend>, name: &str, url: &str) -> Result<()> {
    ensure_ready(repo)?;
    check_remote_name(name)?;
    let url = normalize_remote_url(repo.backend().root(), url)?;
    if repo
        .backend()
        .read_meta(&format!("remotes/{name}"))?
        .is_none()
    {
        return Err(Error::NotFound(format!("remote {name}")));
    }
    let content = serde_json::to_string(&Remote { url })?;
    repo.backend()
        .write_meta(&format!("remotes/{name}"), Some(&content))
}
fn check_remote_name(name: &str) -> Result<()> {
    validate_ref_name(name)?;
    if name.contains('/') {
        return Err(Error::Invalid(
            "remote names must have one component".into(),
        ));
    }
    Ok(())
}
fn ensure_ready(repo: &Repository<FsBackend>) -> Result<()> {
    if repo.backend().read_meta("transaction.json")?.is_some() {
        return Err(Error::Pending);
    }
    Ok(())
}
fn configured(repo: &Repository<FsBackend>, name: &str) -> Result<RemoteAddress> {
    check_remote_name(name)?;
    let remote = remote_list(repo)?
        .remove(name)
        .ok_or_else(|| Error::NotFound(format!("remote {name}")))?;
    RemoteAddress::parse(&remote.url)
}
fn inventory(repo: &Repository<FsBackend>) -> Result<Inventory> {
    ensure_ready(repo)?;
    repo.fsck()?;
    let head = repo.backend().read_meta("HEAD")?.and_then(|s| {
        s.strip_prefix("ref: refs/heads/")
            .and_then(|s| s.strip_suffix('\n'))
            .map(str::to_owned)
    });
    Ok(Inventory {
        version: 1,
        refs: repo.reference_inventory()?,
        objects: repo.backend().list_objects()?,
        head,
    })
}

enum Connection {
    Local(Box<Repository<FsBackend>>),
    Ssh {
        child: Child,
        input: ChildStdin,
        output: BufReader<ChildStdout>,
    },
}
fn quote_shell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
impl Connection {
    fn connect(address: &RemoteAddress, write: bool) -> Result<Self> {
        match address {
            RemoteAddress::Local(path) => Ok(Self::Local(Box::new(Repository::open(
                FsBackend::open(path)?,
            )?))),
            RemoteAddress::Ssh { host, path, .. } | RemoteAddress::Mesh { host, path } => {
                let mesh = matches!(address, RemoteAddress::Mesh { .. });
                let mut command = Command::new(if mesh { "tailscale" } else { "ssh" });
                if mesh {
                    command.arg("ssh");
                } else {
                    command.args([
                        "-T",
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "StrictHostKeyChecking=yes",
                        "-o",
                        "ConnectTimeout=10",
                        "-o",
                        "ServerAliveInterval=15",
                        "-o",
                        "ServerAliveCountMax=3",
                    ]);
                    if let RemoteAddress::Ssh {
                        port: Some(port), ..
                    } = address
                    {
                        command.arg("-p").arg(port.to_string());
                    }
                    command.arg("--");
                }
                let remote_command = format!(
                    "knot -C {} serve --stdio{}",
                    quote_shell(path),
                    if write { " --allow-writes" } else { "" }
                );
                let mut child = command
                    .arg(host)
                    .arg(remote_command)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()?;
                let input = child
                    .stdin
                    .take()
                    .ok_or_else(|| Error::Io(std::io::Error::other("missing SSH stdin")))?;
                let output = BufReader::new(
                    child
                        .stdout
                        .take()
                        .ok_or_else(|| Error::Io(std::io::Error::other("missing SSH stdout")))?,
                );
                Ok(Self::Ssh {
                    child,
                    input,
                    output,
                })
            }
        }
    }
    fn inventory(&mut self) -> Result<Inventory> {
        let inventory = match self {
            Self::Local(repo) => inventory(repo)?,
            Self::Ssh { input, output, .. } => {
                send(input, &Request::Advertise)?;
                serde_json::from_value(reply(output)?)?
            }
        };
        if inventory.version != 1 {
            return Err(Error::Integrity(
                "unsupported synchronization protocol".into(),
            ));
        }
        let ids: BTreeSet<_> = inventory.objects.iter().collect();
        if ids.len() != inventory.objects.len() {
            return Err(Error::Integrity("duplicate advertised objects".into()));
        }
        for name in inventory.refs.keys() {
            if !(name.starts_with("refs/heads/")
                || name.starts_with("refs/tags/")
                || name.starts_with("refs/issues/")
                || name.starts_with("refs/reviews/")
                || name.starts_with("refs/releases/"))
            {
                return Err(Error::Integrity("unexpected advertised namespace".into()));
            }
        }
        Ok(inventory)
    }
    fn download(
        &mut self,
        destination: &Repository<FsBackend>,
        objects: &[ObjectId],
    ) -> Result<()> {
        match self {
            Self::Local(source) => {
                for id in objects {
                    copy_object(source, destination, id)?;
                }
            }
            Self::Ssh { input, output, .. } => {
                send(
                    input,
                    &Request::Get {
                        objects: objects.to_vec(),
                    },
                )?;
                let frames: Vec<ObjectFrame> = serde_json::from_value(reply(output)?)?;
                if frames.iter().map(|f| &f.id).ne(objects.iter()) {
                    return Err(Error::Integrity(
                        "object response differs from request".into(),
                    ));
                }
                for frame in frames {
                    destination
                        .backend()
                        .import_object(&frame.id, frame.len, output)?;
                }
            }
        }
        Ok(())
    }
    fn upload(
        &mut self,
        source: &Repository<FsBackend>,
        objects: &[ObjectId],
        updates: &[ReferenceUpdate],
    ) -> Result<()> {
        match self {
            Self::Local(destination) => {
                for id in objects {
                    copy_object(source, destination, id)?;
                }
                destination.publish_references(updates, None, true)?;
            }
            Self::Ssh { input, output, .. } => {
                let frames = objects
                    .iter()
                    .map(|id| {
                        Ok(ObjectFrame {
                            id: id.clone(),
                            len: source.backend().object_size(id)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                send(
                    input,
                    &Request::Put {
                        objects: frames,
                        updates: updates.to_vec(),
                    },
                )?;
                if reply(output)? != serde_json::json!({"ready":true}) {
                    return Err(Error::Integrity("peer did not accept upload".into()));
                }
                for id in objects {
                    source.backend().export_object(id, input)?;
                }
                input.flush()?;
                reply(output)?;
            }
        }
        Ok(())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Self::Ssh { child, .. } = self {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn copy_object(
    source: &Repository<FsBackend>,
    destination: &Repository<FsBackend>,
    id: &ObjectId,
) -> Result<()> {
    let len = source.backend().object_size(id)?;
    let mut file = File::open(source.backend().object_path(id)?)?;
    destination.backend().import_object(id, len, &mut file)
}
fn send<T: Serialize>(out: &mut impl Write, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 > MAX_CONTROL {
        return Err(Error::Invalid(
            "synchronization control message exceeds 16 MiB".into(),
        ));
    }
    out.write_all(&bytes)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}
fn receive<T: serde::de::DeserializeOwned>(input: &mut impl BufRead) -> Result<Option<T>> {
    let mut line = Vec::new();
    let count = input
        .by_ref()
        .take(MAX_CONTROL + 1)
        .read_until(b'\n', &mut line)?;
    if count == 0 {
        return Ok(None);
    }
    if line.len() as u64 > MAX_CONTROL {
        return Err(Error::Invalid(
            "synchronization control message too large".into(),
        ));
    }
    Ok(Some(serde_json::from_slice(&line)?))
}
fn reply(input: &mut impl BufRead) -> Result<serde_json::Value> {
    let value: serde_json::Value = receive(input)?.ok_or_else(|| {
        Error::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "SSH peer closed the protocol stream",
        ))
    })?;
    if value["ok"] != true {
        return Err(Error::Conflict(format!(
            "peer refused operation: {}",
            value["error"]
        )));
    }
    Ok(value["data"].clone())
}
fn success(out: &mut impl Write, value: impl Serialize) -> Result<()> {
    send(out, &serde_json::json!({"ok":true,"data":value}))
}

/// SSH forced-command endpoint. The OS account/SSH authorized_keys controls repository access.
pub fn serve_repository(
    repo: &Repository<FsBackend>,
    input: &mut impl BufRead,
    out: &mut impl Write,
    allow_writes: bool,
) -> Result<()> {
    while let Some(request) = receive::<Request>(input)? {
        let result = (|| -> Result<()> {
            match request {
                Request::Advertise => success(out, inventory(repo)?),
                Request::Get { objects } => {
                    ensure_ready(repo)?;
                    let frames = objects
                        .iter()
                        .map(|id| {
                            Ok(ObjectFrame {
                                id: id.clone(),
                                len: repo.backend().object_size(id)?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    // Verify every object before sending the first frame.
                    for id in &objects {
                        if repo.backend().object_kind(id)? == ObjectKind::Blob {
                            repo.backend().verify_blob(id)?;
                        } else {
                            repo.backend().get_object(id)?;
                        }
                    }
                    success(out, frames)?;
                    for id in objects {
                        repo.backend().export_object(&id, out)?;
                    }
                    out.flush()?;
                    Ok(())
                }
                Request::Put { objects, updates } => {
                    if !allow_writes {
                        return Err(Error::Conflict("peer endpoint is read-only".into()));
                    }
                    ensure_ready(repo)?;
                    let unique: BTreeSet<_> = objects.iter().map(|f| &f.id).collect();
                    if unique.len() != objects.len() {
                        return Err(Error::Invalid("duplicate upload frames".into()));
                    }
                    success(out, serde_json::json!({"ready":true}))?;
                    for frame in objects {
                        repo.backend().import_object(&frame.id, frame.len, input)?;
                    }
                    repo.publish_references(&updates, None, true)?;
                    success(out, serde_json::json!({"published":updates.len()}))
                }
            }
        })();
        if let Err(error) = result {
            send(
                out,
                &serde_json::json!({"ok":false,"error":{"code":error.code(),"message":error.to_string(),"details":error.details()}}),
            )?;
            // An interrupted raw-object stream cannot be safely reframed.
            return Ok(());
        }
    }
    Ok(())
}

fn downloads(local: &Repository<FsBackend>, inventory: &Inventory) -> Result<Vec<ObjectId>> {
    let have: BTreeSet<_> = local.backend().list_objects()?.into_iter().collect();
    Ok(inventory
        .objects
        .iter()
        .filter(|id| !have.contains(*id))
        .cloned()
        .collect())
}
fn updates(
    local: &Repository<FsBackend>,
    refs: impl IntoIterator<Item = (String, ObjectId)>,
) -> Result<Vec<ReferenceUpdate>> {
    refs.into_iter()
        .map(|(name, after)| {
            let before = local
                .backend()
                .read_meta(&name)?
                .map(|s| s.trim_end_matches('\n').parse())
                .transpose()?;
            Ok(ReferenceUpdate {
                name,
                before,
                after,
            })
        })
        .collect()
}
pub fn fetch(repo: &Repository<FsBackend>, name: &str) -> Result<TransferReport> {
    ensure_ready(repo)?;
    repo.fsck()?;
    let mut peer = Connection::connect(&configured(repo, name)?, false)?;
    let inventory = peer.inventory()?;
    let missing = downloads(repo, &inventory)?;
    peer.download(repo, &missing)?;
    let refs = inventory.refs.into_iter().map(|(name_ref, id)| {
        (
            if let Some(branch) = name_ref.strip_prefix("refs/heads/") {
                format!("refs/remotes/{name}/{branch}")
            } else {
                name_ref
            },
            id,
        )
    });
    let updates = updates(repo, refs)?;
    repo.publish_references(&updates, None, true)?;
    Ok(TransferReport {
        objects: missing.len(),
        references: updates.len(),
        remote: name.into(),
    })
}
pub fn push(
    repo: &Repository<FsBackend>,
    name: &str,
    branch: Option<&str>,
) -> Result<TransferReport> {
    ensure_ready(repo)?;
    repo.fsck()?;
    let branch = branch
        .map(str::to_owned)
        .or(repo.status()?.branch)
        .ok_or_else(|| Error::Invalid("push from detached HEAD requires a branch name".into()))?;
    validate_ref_name(&branch)?;
    let branch_ref = format!("refs/heads/{branch}");
    let refs: Vec<_> = repo
        .reference_inventory()?
        .into_iter()
        .filter(|(name, _)| name == &branch_ref || !name.starts_with("refs/heads/"))
        .collect();
    if !refs.iter().any(|(name, _)| name == &branch_ref) {
        return Err(Error::NotFound(format!("branch {branch}")));
    }
    let mut peer = Connection::connect(&configured(repo, name)?, true)?;
    let inventory = peer.inventory()?;
    let have: BTreeSet<_> = inventory.objects.into_iter().collect();
    let missing: Vec<_> = repo
        .backend()
        .list_objects()?
        .into_iter()
        .filter(|id| !have.contains(id))
        .collect();
    let updates = refs
        .into_iter()
        .map(|(name, after)| ReferenceUpdate {
            before: inventory.refs.get(&name).cloned(),
            name,
            after,
        })
        .collect::<Vec<_>>();
    peer.upload(repo, &missing, &updates)?;
    Ok(TransferReport {
        objects: missing.len(),
        references: updates.len(),
        remote: name.into(),
    })
}
pub fn pull(
    repo: &Repository<FsBackend>,
    name: &str,
    allow_incompatible: bool,
) -> Result<TransferReport> {
    let report = fetch(repo, name)?;
    let status = repo.status()?;
    let branch = status
        .branch
        .ok_or_else(|| Error::Conflict("pull requires an attached branch".into()))?;
    let remote = repo
        .backend()
        .read_meta(&format!("refs/remotes/{name}/{branch}"))?
        .ok_or_else(|| Error::NotFound("peer branch".into()))?;
    let after = remote.trim_end_matches('\n').parse()?;
    let refs = updates(repo, [(format!("refs/heads/{branch}"), after)])?;
    repo.publish_references(&refs, Some(&branch), allow_incompatible)?;
    Ok(report)
}
pub fn clone_repository(
    url: &str,
    destination: &Path,
    branch: Option<&str>,
    allow_incompatible: bool,
) -> Result<TransferReport> {
    let url = normalize_remote_url(&std::env::current_dir()?, url)?;
    let address = RemoteAddress::parse(&url)?;
    if destination.exists() && fs::read_dir(destination)?.next().is_some() {
        return Err(Error::Conflict("clone destination must be empty".into()));
    }
    let mut peer = Connection::connect(&address, false)?;
    let inventory = peer.inventory()?;
    if let Some(branch) = branch {
        validate_ref_name(branch)?;
        if !inventory.refs.contains_key(&format!("refs/heads/{branch}")) {
            return Err(Error::NotFound(format!("peer branch {branch}")));
        }
    }
    let local = Repository::open(FsBackend::init(destination, None)?)?;
    let missing = downloads(&local, &inventory)?;
    peer.download(&local, &missing)?;
    let branch = branch
        .map(str::to_owned)
        .or(inventory.head.clone())
        .filter(|name| inventory.refs.contains_key(&format!("refs/heads/{name}")))
        .or_else(|| {
            inventory
                .refs
                .keys()
                .find_map(|name| name.strip_prefix("refs/heads/").map(str::to_owned))
        });
    let mut refs = inventory.refs.clone();
    for (name, id) in &inventory.refs {
        if let Some(branch) = name.strip_prefix("refs/heads/") {
            refs.insert(format!("refs/remotes/origin/{branch}"), id.clone());
        }
    }
    let updates = updates(&local, refs)?;
    local.publish_references(&updates, branch.as_deref(), allow_incompatible)?;
    remote_add(&local, "origin", &url)?;
    Ok(TransferReport {
        objects: missing.len(),
        references: updates.len(),
        remote: url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_addresses_are_data_and_shell_paths_are_quoted() {
        assert_eq!(
            RemoteAddress::parse("ssh://user@host:2222/a b/it's").ok(),
            Some(RemoteAddress::Ssh {
                host: "user@host".into(),
                port: Some(2222),
                path: "/a b/it's".into()
            })
        );
        assert_eq!(quote_shell("/a'$(echo x)"), "'/a'\\''$(echo x)'");
        for url in [
            "ssh://-oProxyCommand=x/path",
            "ssh://user@-host/path",
            "ssh://user@host:0/path",
            "https://example/repo",
            "ssh://host",
        ] {
            assert!(RemoteAddress::parse(url).is_err());
        }
    }
}
