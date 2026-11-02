use crate::filesystem::{FsBackend, checked_join, sync_directory, sync_directory_chain};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use knot_core::{Error, FileState, Result, validate_path, validate_path_set};
use std::{fs, path::Path};

impl FsBackend {
    pub(crate) fn working_files(&self, include_ignored: bool) -> Result<Vec<String>> {
        let ignore = if include_ignored {
            None
        } else {
            let mut builder = GitignoreBuilder::new(&self.root);
            let ignore_path = checked_join(&self.root, ".knotignore")?;
            if ignore_path.exists()
                && let Some(error) = builder.add(ignore_path)
            {
                return Err(Error::Invalid(format!("invalid .knotignore: {error}")));
            }
            Some(builder.build().map_err(|e| Error::Invalid(e.to_string()))?)
        };
        let mut files = Vec::new();
        self.walk_worktree(&self.root, ignore.as_ref(), &mut files)?;
        files.sort();
        validate_path_set(files.iter().map(String::as_str))?;
        Ok(files)
    }
    fn walk_worktree(
        &self,
        dir: &Path,
        ignore: Option<&Gitignore>,
        out: &mut Vec<String>,
    ) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::Invalid("non-UTF-8 worktree path".into()))?;
            if [".knot", ".git"].contains(&name.to_lowercase().as_str())
                || (dir == self.root && name.starts_with(".knot-init-"))
            {
                continue;
            }
            let file_type = entry.file_type()?;
            let path = entry.path();
            if ignore.is_some_and(|i| {
                i.matched_path_or_any_parents(&path, file_type.is_dir())
                    .is_ignore()
            }) {
                continue;
            }
            let relative = path
                .strip_prefix(&self.root)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let relative = relative
                .iter()
                .map(|p| {
                    p.to_str()
                        .ok_or_else(|| Error::Invalid("non-UTF-8 worktree path".into()))
                })
                .collect::<Result<Vec<_>>>()?
                .join("/");
            validate_path(&relative)?;
            if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
                return Err(Error::Invalid(format!(
                    "unsupported symlink or special file: {relative}"
                )));
            }
            if file_type.is_dir() {
                self.walk_worktree(&path, ignore, out)?;
            } else {
                out.push(relative);
            }
        }
        Ok(())
    }
    pub(crate) fn working_state(
        &self,
        relative: &str,
        executable_hint: bool,
        persist: bool,
    ) -> Result<Option<FileState>> {
        validate_path(relative)?;
        let path = checked_join(&self.root, relative)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        if metadata.is_dir() {
            return Ok(None);
        }
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Error::Invalid(format!("unsupported file: {relative}")));
        }
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            let _ = executable_hint;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = executable_hint;
        let blob = self.import_file(&path, persist)?;
        Ok(Some(FileState { blob, executable }))
    }
    pub(crate) fn restore_file(&self, relative: &str, state: Option<&FileState>) -> Result<()> {
        validate_path(relative)?;
        let path = checked_join(&self.root, relative)?;
        if let Some(state) = state {
            // Empty directories are not versioned. Never remove a directory with files.
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => fs::remove_dir(&path).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                        Error::Conflict(format!(
                            "nonempty directory blocks restoration: {relative}"
                        ))
                    } else {
                        Error::Io(e)
                    }
                })?,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            let parent = path
                .parent()
                .ok_or_else(|| Error::Invalid("missing worktree parent".into()))?;
            fs::create_dir_all(parent)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            self.stream_blob(&state.blob, temporary.as_file_mut())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                temporary
                    .as_file()
                    .set_permissions(fs::Permissions::from_mode(if state.executable {
                        0o755
                    } else {
                        0o644
                    }))?;
            }
            temporary.as_file().sync_all()?;
            temporary.persist(&path).map_err(|e| Error::Io(e.error))?;
            sync_directory_chain(parent, &self.root)?;
        } else {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.into()),
            }
            if let Some(parent) = path.parent() {
                sync_directory_chain(parent, &self.root)?;
            }
            let mut parent = path.parent();
            while let Some(dir) = parent {
                if dir == self.root {
                    break;
                }
                match fs::remove_dir(dir) {
                    Ok(()) => {
                        if let Some(ancestor) = dir.parent() {
                            sync_directory(ancestor)?;
                        }
                    }
                    Err(_) => break,
                }
                parent = dir.parent();
            }
        }
        Ok(())
    }
}
