use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

pub fn portable_key(path: &str) -> String {
    path.nfc().case_fold().collect()
}

pub fn validate_path(path: &str) -> Result<()> {
    if path
        .split('/')
        .next()
        .is_some_and(|p| p.starts_with(".knot-init-"))
    {
        return Err(Error::Invalid(
            "initialization staging paths are reserved metadata".into(),
        ));
    }
    if path.is_empty() || path.len() > 240 || path.nfc().collect::<String>() != path {
        return Err(Error::Invalid(format!(
            "path must be NFC UTF-8 and at most 240 bytes: {path}"
        )));
    }
    for part in path.split('/') {
        let upper = part.to_uppercase();
        let stem = upper.split('.').next().unwrap_or("");
        let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"];
        let numbered_device = (stem.starts_with("COM") || stem.starts_with("LPT"))
            && (stem[3..].chars().count() == 1)
            && stem[3..].chars().any(|c| "123456789¹²³".contains(c));
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part
                .bytes()
                .any(|b| b < 32 || b == 127 || b"\\:<>\"|?*".contains(&b))
            || reserved.contains(&stem)
            || numbered_device
            || [".knot", ".git"].contains(&part.to_lowercase().as_str())
        {
            return Err(Error::Invalid(format!(
                "nonportable or reserved path: {path}"
            )));
        }
    }
    Ok(())
}

pub fn validate_path_set<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut keys = BTreeSet::new();
    let mut spellings = BTreeMap::new();
    for path in paths {
        validate_path(path)?;
        for prefix in path
            .match_indices('/')
            .map(|(i, _)| &path[..i])
            .chain(std::iter::once(path))
        {
            let folded = portable_key(prefix);
            if let Some(existing) = spellings.insert(folded, prefix.to_owned())
                && existing != prefix
            {
                return Err(Error::Integrity(format!(
                    "case-colliding path components: {existing} and {prefix}"
                )));
            }
        }
        let key = portable_key(path);
        if !keys.insert(key.clone()) {
            return Err(Error::Integrity(format!(
                "duplicate or case-colliding path: {path}"
            )));
        }
    }
    for key in &keys {
        for (position, _) in key.match_indices('/') {
            if keys.contains(&key[..position]) {
                return Err(Error::Integrity(format!("file/directory collision: {key}")));
            }
        }
    }
    Ok(())
}

pub fn validate_ref_name(name: &str) -> Result<()> {
    validate_path(name)?;
    if name == "HEAD"
        || name
            .split('/')
            .any(|p| p.starts_with('.') || p.ends_with(".lock"))
        || name.contains("..")
        || name.contains('@')
        || name.len() == 64
    {
        return Err(Error::Invalid(format!("invalid ref name: {name}")));
    }
    Ok(())
}
