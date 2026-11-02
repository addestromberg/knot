use crate::filesystem::{FsBackend, checked_join, sync_directory_chain};
use knot_core::{
    Error, ObjectId, ObjectKind, Result, object_header, object_header_versioned,
    object_id_versioned,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

fn validate_payload(kind: ObjectKind, version: u32, bytes: &[u8]) -> Result<()> {
    fn check<T: knot_core::StoredObject>(version: u32, bytes: &[u8]) -> Result<()> {
        if !T::SUPPORTED_VERSIONS.contains(&version) {
            return Err(Error::Integrity("unsupported transferred schema".into()));
        }
        let value: T = serde_json::from_slice(bytes)?;
        if value.schema_version() != version || knot_core::canonical(&value)? != bytes {
            return Err(Error::Integrity("noncanonical transferred object".into()));
        }
        value.validate()
    }
    match kind {
        ObjectKind::Asset => check::<knot_core::Asset>(version, bytes),
        ObjectKind::Revision => check::<knot_core::Revision>(version, bytes),
        ObjectKind::Snapshot => check::<knot_core::Snapshot>(version, bytes),
        ObjectKind::Commit => check::<knot_core::Commit>(version, bytes),
        ObjectKind::IssueEvent => check::<knot_core::IssueEvent>(version, bytes),
        ObjectKind::Interface => check::<knot_core::Interface>(version, bytes),
        ObjectKind::Dependency => check::<knot_core::Dependency>(version, bytes),
        ObjectKind::ValidationRecord => check::<knot_core::ValidationRecord>(version, bytes),
        ObjectKind::TestDefinition => check::<knot_core::TestDefinition>(version, bytes),
        ObjectKind::TraceLink => check::<knot_core::TraceLink>(version, bytes),
        ObjectKind::Release => check::<knot_core::Release>(version, bytes),
        ObjectKind::Blob => Ok(()),
    }
}

impl FsBackend {
    /// Export the canonical envelope and payload, streaming blobs without buffering.
    pub fn export_object(&self, id: &ObjectId, out: &mut impl Write) -> Result<u64> {
        let (mut file, kind, _, len, header) = self.open_object(id)?;
        if kind == ObjectKind::Blob {
            self.check_blob(id)?;
        } else {
            self.read_object(id)?;
        }
        file.seek(SeekFrom::Start(0))?;
        let total = header.len() as u64 + len;
        if std::io::copy(&mut file.take(total), out)? != total {
            return Err(Error::Integrity("object changed while exporting".into()));
        }
        Ok(total)
    }
    pub fn object_size(&self, id: &ObjectId) -> Result<u64> {
        let (_, _, _, len, header) = self.open_object(id)?;
        Ok(len + header.len() as u64)
    }
    /// Import an exact framed object, verify its hash/header/schema, then publish atomically.
    pub fn import_object(&self, id: &ObjectId, len: u64, input: &mut impl Read) -> Result<()> {
        let mut temporary =
            tempfile::NamedTempFile::new_in(checked_join(&self.meta, "objects/sha256")?)?;
        let mut remaining = len;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        while remaining > 0 {
            let wanted = remaining.min(buffer.len() as u64) as usize;
            let count = input.read(&mut buffer[..wanted])?;
            if count == 0 {
                return Err(Error::Integrity("truncated object transfer".into()));
            }
            temporary.write_all(&buffer[..count])?;
            hash.update(&buffer[..count]);
            remaining -= count as u64;
        }
        if ObjectId::from_digest(hash.finalize().into()) != *id {
            return Err(Error::Integrity(format!(
                "transferred object {id} has wrong hash"
            )));
        }
        temporary.as_file_mut().seek(SeekFrom::Start(0))?;
        let mut header = Vec::new();
        let mut separators = 0;
        while separators < 4 {
            let mut byte = [0];
            temporary.as_file_mut().read_exact(&mut byte)?;
            header.push(byte[0]);
            if byte[0] == 0 {
                separators += 1;
            }
            if header.len() > 128 {
                return Err(Error::Integrity("invalid transferred object header".into()));
            }
        }
        let fields: Vec<_> = header.split(|b| *b == 0).collect();
        let field = |index: usize| {
            std::str::from_utf8(fields[index]).map_err(|e| Error::Integrity(e.to_string()))
        };
        let kind = ObjectKind::parse(field(1)?)?;
        let version: u32 = field(2)?
            .parse()
            .map_err(|_| Error::Integrity("invalid object version".into()))?;
        let payload_len: u64 = field(3)?
            .parse()
            .map_err(|_| Error::Integrity("invalid object length".into()))?;
        if fields[0] != b"knot"
            || object_header_versioned(kind, version, payload_len) != header
            || (header.len() as u64).checked_add(payload_len) != Some(len)
        {
            return Err(Error::Integrity(
                "noncanonical transferred object envelope".into(),
            ));
        }
        if kind == ObjectKind::Blob {
            if version != 1 {
                return Err(Error::Integrity("unsupported blob schema".into()));
            }
        } else {
            if payload_len > knot_core::MAX_STRUCTURED_OBJECT_SIZE as u64 {
                return Err(Error::Integrity("structured object too large".into()));
            }
            let mut bytes = Vec::new();
            temporary.as_file_mut().read_to_end(&mut bytes)?;
            validate_payload(kind, version, &bytes)?;
        }
        if matches!(
            kind,
            ObjectKind::ValidationRecord
                | ObjectKind::TestDefinition
                | ObjectKind::TraceLink
                | ObjectKind::Release
        ) || version == 2 && matches!(kind, ObjectKind::Dependency | ObjectKind::Commit)
        {
            self.ensure_format(3)?;
        } else if version == 2 || matches!(kind, ObjectKind::Dependency | ObjectKind::Interface) {
            self.ensure_m2_format()?;
        }
        let destination = self.object_path(id)?;
        if destination.exists() {
            if kind == ObjectKind::Blob {
                self.check_blob(id)?;
            } else {
                self.read_object(id)?;
            }
            return Ok(());
        }
        let parent = destination
            .parent()
            .ok_or_else(|| Error::Integrity("missing object parent".into()))?;
        fs::create_dir_all(parent)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(&destination)
            .map_err(|e| Error::Io(e.error))?;
        sync_directory_chain(parent, &self.meta)
    }
    pub(crate) fn object_path(&self, id: &ObjectId) -> Result<std::path::PathBuf> {
        let text = id.as_str();
        checked_join(
            &self.meta,
            &format!("objects/sha256/{}/{}", &text[..2], &text[2..]),
        )
    }
    pub(crate) fn write_object(&self, kind: ObjectKind, payload: &[u8]) -> Result<ObjectId> {
        self.write_versioned_object(kind, 1, payload)
    }
    pub(crate) fn write_versioned_object(
        &self,
        kind: ObjectKind,
        version: u32,
        payload: &[u8],
    ) -> Result<ObjectId> {
        if version != 1
            && !(version == 2
                && matches!(
                    kind,
                    ObjectKind::Revision | ObjectKind::Dependency | ObjectKind::Commit
                ))
        {
            return Err(Error::Integrity("unsupported object schema".into()));
        }
        if kind != ObjectKind::Blob && payload.len() > knot_core::MAX_STRUCTURED_OBJECT_SIZE {
            return Err(Error::Invalid(
                "structured object exceeds 16 MiB limit".into(),
            ));
        }
        if matches!(
            kind,
            ObjectKind::ValidationRecord
                | ObjectKind::TestDefinition
                | ObjectKind::TraceLink
                | ObjectKind::Release
        ) || version == 2 && matches!(kind, ObjectKind::Dependency | ObjectKind::Commit)
        {
            self.ensure_format(3)?;
        } else if version == 2 || matches!(kind, ObjectKind::Dependency | ObjectKind::Interface) {
            self.ensure_m2_format()?;
        }
        let id = object_id_versioned(kind, version, payload);
        let path = self.object_path(&id)?;
        if path.exists() {
            if kind == ObjectKind::Blob {
                self.check_blob(&id)?;
                return Ok(id);
            }
            let (actual_kind, actual) = self.read_object(&id)?;
            if actual_kind != kind || actual != payload {
                return Err(Error::Integrity(
                    "existing object differs from requested contents".into(),
                ));
            }
            return Ok(id);
        }
        let mut bytes = object_header_versioned(kind, version, payload.len() as u64);
        bytes.extend_from_slice(payload);
        self.atomic_write(&path, &bytes)?;
        Ok(id)
    }
    pub(crate) fn open_object(
        &self,
        id: &ObjectId,
    ) -> Result<(File, ObjectKind, u32, u64, Vec<u8>)> {
        let mut file = File::open(self.object_path(id)?).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Integrity(format!("missing object {id}"))
            } else {
                e.into()
            }
        })?;
        let mut header = Vec::new();
        let mut separators = 0;
        while separators < 4 {
            let mut byte = [0u8];
            file.read_exact(&mut byte)
                .map_err(|e| Error::Integrity(format!("invalid object header {id}: {e}")))?;
            if byte[0] == 0 {
                separators += 1;
            }
            header.push(byte[0]);
            if header.len() > 128 {
                return Err(Error::Integrity("object header too long".into()));
            }
        }
        let fields: Vec<_> = header.split(|b| *b == 0).collect();
        if fields[0] != b"knot" {
            return Err(Error::Integrity(
                "unsupported object magic or schema version".into(),
            ));
        }
        let kind = ObjectKind::parse(
            std::str::from_utf8(fields[1]).map_err(|e| Error::Integrity(e.to_string()))?,
        )?;
        let version: u32 = std::str::from_utf8(fields[2])
            .map_err(|e| Error::Integrity(e.to_string()))?
            .parse()
            .map_err(|_| Error::Integrity("invalid schema version".into()))?;
        if version != 1
            && !(version == 2
                && matches!(
                    kind,
                    ObjectKind::Revision | ObjectKind::Dependency | ObjectKind::Commit
                ))
        {
            return Err(Error::Integrity("unsupported object schema version".into()));
        }
        let len: u64 = std::str::from_utf8(fields[3])
            .map_err(|e| Error::Integrity(e.to_string()))?
            .parse()
            .map_err(|_| Error::Integrity("invalid object size".into()))?;
        if object_header_versioned(kind, version, len) != header
            || Some(file.metadata()?.len()) != (header.len() as u64).checked_add(len)
        {
            return Err(Error::Integrity(format!(
                "{id}: noncanonical header or incorrect size"
            )));
        }
        Ok((file, kind, version, len, header))
    }
    pub(crate) fn read_object(&self, id: &ObjectId) -> Result<(ObjectKind, Vec<u8>)> {
        let (mut file, kind, _, len, header) = self.open_object(id)?;
        if len > knot_core::MAX_STRUCTURED_OBJECT_SIZE as u64 {
            return Err(Error::Integrity(
                "object exceeds buffered-read limit; use streaming blob operations".into(),
            ));
        }
        let mut payload = Vec::new();
        file.read_to_end(&mut payload)?;
        let mut hash = Sha256::new();
        hash.update(header);
        hash.update(&payload);
        if ObjectId::from_digest(hash.finalize().into()) != *id || payload.len() as u64 != len {
            return Err(Error::Integrity(format!("{id}: hash or length mismatch")));
        }
        Ok((kind, payload))
    }
    pub(crate) fn check_blob(&self, id: &ObjectId) -> Result<u64> {
        self.stream_blob(id, &mut std::io::sink())
    }
    pub(crate) fn stream_blob(&self, id: &ObjectId, out: &mut impl Write) -> Result<u64> {
        let (mut file, kind, _, len, header) = self.open_object(id)?;
        if kind != ObjectKind::Blob {
            return Err(Error::Integrity(format!("{id}: expected blob")));
        }
        let mut hash = Sha256::new();
        hash.update(header);
        let mut buffer = [0u8; 65536];
        let mut count = 0u64;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
            out.write_all(&buffer[..read])?;
            count += read as u64;
        }
        if ObjectId::from_digest(hash.finalize().into()) != *id || count != len {
            return Err(Error::Integrity(format!("{id}: hash or length mismatch")));
        }
        Ok(count)
    }
    pub(crate) fn publish_blob(
        &self,
        temporary: tempfile::NamedTempFile,
        id: &ObjectId,
    ) -> Result<()> {
        let path = self.object_path(id)?;
        if path.exists() {
            self.check_blob(id)?;
            return Ok(());
        }
        let parent = path
            .parent()
            .ok_or_else(|| Error::Integrity("missing object parent".into()))?;
        fs::create_dir_all(parent)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(&path)
            .map_err(|e| Error::Io(e.error))?;
        sync_directory_chain(parent, &self.meta)?;
        Ok(())
    }
    pub(crate) fn object_ids(&self) -> Result<Vec<ObjectId>> {
        let root = checked_join(&self.meta, "objects/sha256")?;
        let mut out = Vec::new();
        for fanout in fs::read_dir(&root)? {
            let fanout = fanout?;
            if fanout.file_type()?.is_file()
                && fanout.file_name().to_string_lossy().starts_with(".tmp")
            {
                continue;
            }
            if !fanout.file_type()?.is_dir() {
                return Err(Error::Integrity(
                    "unexpected file in object fanout root".into(),
                ));
            }
            let prefix = fanout
                .file_name()
                .into_string()
                .map_err(|_| Error::Integrity("non-UTF-8 object path".into()))?;
            if prefix.len() != 2
                || !prefix
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::Integrity("invalid object fanout".into()));
            }
            for entry in fs::read_dir(fanout.path())? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    return Err(Error::Integrity(
                        "unexpected non-file in object storage".into(),
                    ));
                }
                let suffix = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Integrity("non-UTF-8 object path".into()))?;
                if suffix.starts_with(".tmp") {
                    continue;
                }
                out.push(
                    format!("{prefix}{suffix}")
                        .parse()
                        .map_err(|_| Error::Integrity("invalid object filename".into()))?,
                );
            }
        }
        out.sort();
        Ok(out)
    }
    pub(crate) fn import_file(&self, path: &Path, persist: bool) -> Result<ObjectId> {
        let mut source = File::open(path)?;
        let initial = source.metadata()?;
        let len = initial.len();
        let header = object_header(ObjectKind::Blob, len);
        let mut hash = Sha256::new();
        hash.update(&header);
        let object_root = checked_join(&self.meta, "objects/sha256")?;
        let mut temporary = if persist {
            Some(tempfile::NamedTempFile::new_in(object_root)?)
        } else {
            None
        };
        if let Some(ref mut temporary) = temporary {
            temporary.write_all(&header)?;
        }
        let mut buffer = [0u8; 65536];
        let mut count = 0u64;
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
            count += read as u64;
            if let Some(ref mut temporary) = temporary {
                temporary.write_all(&buffer[..read])?;
            }
        }
        let final_meta = source.metadata()?;
        if count != len || initial.modified()? != final_meta.modified()? || final_meta.len() != len
        {
            return Err(Error::Conflict(format!(
                "{} changed while reading",
                path.display()
            )));
        }
        let id = ObjectId::from_digest(hash.finalize().into());
        if let Some(temporary) = temporary {
            self.publish_blob(temporary, &id)?;
        }
        Ok(id)
    }
}
