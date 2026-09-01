//! Durable JSON documents (reference/data-structures.md): every persisted record
//! is written **atomically** (a sibling temp file renamed over the target), so a
//! crash mid-write can never leave a half-written document behind. One helper,
//! used by every subsystem that persists state (DRY).

use crate::error::{ClatchError, Result};
use serde::Serialize;
use std::path::Path;

/// Serialize `value` (pretty JSON) and write it to `path` atomically: parent dirs
/// are created, the bytes land in a sibling `.tmp` file, and a rename publishes
/// the document in one step (rename is atomic on the same filesystem).
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let json =
        serde_json::to_string_pretty(value).map_err(|e| ClatchError::Invalid(e.to_string()))?;
    let parent = path
        .parent()
        .ok_or_else(|| ClatchError::Invalid(format!("{}: no parent dir", path.display())))?;
    std::fs::create_dir_all(parent).map_err(|e| ClatchError::io(parent, e))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(|e| ClatchError::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| ClatchError::io(path, e))
}

/// Read + parse one JSON document. `Ok(None)` only for a **missing** file; a
/// document that exists but fails to read or parse is an error, never silently
/// treated as absent (reference/data-structures.md).
pub fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let json = match std::fs::read_to_string(path) {
        Ok(json) => json,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ClatchError::io(path, e)),
    };
    serde_json::from_str(&json)
        .map(Some)
        .map_err(|e| ClatchError::Invalid(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Doc {
        n: u32,
    }

    #[test]
    fn round_trip_and_missing_vs_corrupt() {
        let dir = std::env::temp_dir().join(format!("clatch-persist-{}", std::process::id()));
        let path = dir.join("nested").join("doc.json");

        // Missing is None, not an error.
        assert!(load_json::<Doc>(&path).unwrap().is_none());

        // Save creates parents, publishes atomically, and no temp file survives.
        save_json(&path, &Doc { n: 7 }).unwrap();
        assert!(!path.with_extension("tmp").exists());
        assert_eq!(load_json::<Doc>(&path).unwrap(), Some(Doc { n: 7 }));

        // Corrupt is an error, never a silent default.
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load_json::<Doc>(&path).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
