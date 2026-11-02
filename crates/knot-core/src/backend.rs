use crate::{ObjectId, ObjectKind, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileState {
    pub blob: ObjectId,
    pub executable: bool,
}

/// A backend holds an exclusive repository lock for its entire lifetime.
/// Paths are portable, root-relative, and must never follow symbolic links.
pub trait Backend {
    fn is_bare(&self) -> bool {
        false
    }
    fn author(&self) -> Result<Option<String>>;
    fn read_meta(&self, name: &str) -> Result<Option<String>>;
    fn write_meta(&self, name: &str, content: Option<&str>) -> Result<()>;
    fn list_meta(&self, prefix: &str) -> Result<Vec<String>>;
    fn put_object(&self, kind: ObjectKind, payload: &[u8]) -> Result<ObjectId>;
    fn put_versioned_object(
        &self,
        kind: ObjectKind,
        version: u32,
        payload: &[u8],
    ) -> Result<ObjectId>;
    fn object_version(&self, id: &ObjectId) -> Result<u32>;
    fn get_object(&self, id: &ObjectId) -> Result<(ObjectKind, Vec<u8>)>;
    fn object_kind(&self, id: &ObjectId) -> Result<ObjectKind>;
    fn verify_blob(&self, id: &ObjectId) -> Result<u64>;
    fn list_objects(&self) -> Result<Vec<ObjectId>>;
    fn list_files(&self, include_ignored: bool) -> Result<Vec<String>>;
    fn file_state(&self, path: &str, executable_hint: bool) -> Result<Option<FileState>>;
    fn store_file(&self, path: &str, executable_hint: bool) -> Result<Option<FileState>>;
    fn write_file(&self, path: &str, state: Option<&FileState>) -> Result<()>;
}
