use crate::{Error, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{fmt, str::FromStr};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ObjectId(String);

impl ObjectId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn from_digest(bytes: [u8; 32]) -> Self {
        Self(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
}
impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl FromStr for ObjectId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        if s.len() != 64
            || !s
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid(format!("invalid SHA-256 object ID: {s}")));
        }
        Ok(Self(s.to_owned()))
    }
}
impl TryFrom<String> for ObjectId {
    type Error = Error;
    fn try_from(s: String) -> Result<Self> {
        s.parse()
    }
}
impl From<ObjectId> for String {
    fn from(id: ObjectId) -> Self {
        id.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Blob,
    Asset,
    Revision,
    Snapshot,
    Commit,
    IssueEvent,
    Interface,
    Dependency,
}
impl ObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Asset => "asset",
            Self::Revision => "revision",
            Self::Snapshot => "snapshot",
            Self::Commit => "commit",
            Self::IssueEvent => "issue_event",
            Self::Interface => "interface",
            Self::Dependency => "dependency",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "blob" => Ok(Self::Blob),
            "asset" => Ok(Self::Asset),
            "revision" => Ok(Self::Revision),
            "snapshot" => Ok(Self::Snapshot),
            "commit" => Ok(Self::Commit),
            "issue_event" => Ok(Self::IssueEvent),
            "interface" => Ok(Self::Interface),
            "dependency" => Ok(Self::Dependency),
            _ => Err(Error::Integrity(format!("unsupported object type: {s}"))),
        }
    }
}

pub fn object_header(kind: ObjectKind, len: u64) -> Vec<u8> {
    object_header_versioned(kind, 1, len)
}
pub fn object_header_versioned(kind: ObjectKind, version: u32, len: u64) -> Vec<u8> {
    format!("knot\0{}\0{version}\0{len}\0", kind.as_str()).into_bytes()
}
pub fn object_id(kind: ObjectKind, payload: &[u8]) -> ObjectId {
    object_id_versioned(kind, 1, payload)
}
pub fn object_id_versioned(kind: ObjectKind, version: u32, payload: &[u8]) -> ObjectId {
    let mut hash = Sha256::new();
    hash.update(object_header_versioned(kind, version, payload.len() as u64));
    hash.update(payload);
    ObjectId::from_digest(hash.finalize().into())
}
pub fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(serde_json_canonicalizer::to_vec(value)?)
}

pub const MAX_STRUCTURED_OBJECT_SIZE: usize = 16 * 1024 * 1024;

pub trait StoredObject: Serialize + DeserializeOwned {
    const KIND: ObjectKind;
    const SUPPORTED_VERSIONS: &'static [u32] = &[1];
    fn schema_version(&self) -> u32 {
        1
    }
    fn validate(&self) -> Result<()>;
}

pub(crate) fn validate_timestamp(value: &str) -> Result<()> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| Error::Integrity("invalid UTC timestamp".into()))?;
    if parsed.to_string() != value {
        return Err(Error::Integrity("noncanonical UTC timestamp".into()));
    }
    Ok(())
}
fn validate_parents(parents: &[ObjectId]) -> Result<()> {
    if parents
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != parents.len()
    {
        return Err(Error::Integrity("duplicate ancestry parent".into()));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub identity: Uuid,
}
impl StoredObject for Asset {
    const KIND: ObjectKind = ObjectKind::Asset;
    fn validate(&self) -> Result<()> {
        if self.identity.get_version_num() != 4 {
            return Err(Error::Integrity("asset identity must be UUIDv4".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub asset: ObjectId,
    pub blob: ObjectId,
    pub parents: Vec<ObjectId>,
    pub executable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<ObjectId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<ObjectId>,
}
impl StoredObject for Revision {
    const KIND: ObjectKind = ObjectKind::Revision;
    const SUPPORTED_VERSIONS: &'static [u32] = &[1, 2];
    fn schema_version(&self) -> u32 {
        if self.dependencies.is_empty() && self.interfaces.is_empty() {
            1
        } else {
            2
        }
    }
    fn validate(&self) -> Result<()> {
        validate_parents(&self.parents)?;
        if self.dependencies.windows(2).any(|p| p[0] >= p[1])
            || self.interfaces.windows(2).any(|p| p[0] >= p[1])
        {
            return Err(Error::Integrity(
                "revision metadata references must be sorted and unique".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotEntry {
    pub path: String,
    pub revision: ObjectId,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub entries: Vec<SnapshotEntry>,
}
impl StoredObject for Snapshot {
    const KIND: ObjectKind = ObjectKind::Snapshot;
    fn validate(&self) -> Result<()> {
        crate::validate_path_set(self.entries.iter().map(|e| e.path.as_str()))
            .map_err(|e| Error::Integrity(e.to_string()))?;
        if self.entries.windows(2).any(|e| e[0].path >= e[1].path) {
            return Err(Error::Integrity(
                "snapshot entries must be sorted by UTF-8 path".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Commit {
    pub snapshot: ObjectId,
    pub parents: Vec<ObjectId>,
    pub message: String,
    pub author: Option<String>,
    /// UTC seconds since Unix epoch, represented as a decimal string.
    pub timestamp: String,
}
impl StoredObject for Commit {
    const KIND: ObjectKind = ObjectKind::Commit;
    fn validate(&self) -> Result<()> {
        if self.message.trim().is_empty() {
            return Err(Error::Integrity("commit message must not be empty".into()));
        }
        validate_timestamp(&self.timestamp)?;
        validate_parents(&self.parents)
    }
}
