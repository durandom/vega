//! Small file helpers for adapters: atomic writes that keep the file's mode
//! and write through a symlink (a dotfiles link stays a link), and one-key
//! edits of JSON objects that keep every other key and its order.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The real file behind `path` (dotfiles managers link files).
fn target(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Replaces `path`'s content via a temp file in the same directory and a
/// rename, so a reader never sees half a file.
pub fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let path = target(path);
    let dir = path.parent().ok_or("no parent directory")?;
    let mode = fs::metadata(&path).ok().map(|m| m.permissions().mode());
    let tmp = dir.join(format!(
        ".{}.vega-theme.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        if let Some(m) = mode {
            fs::set_permissions(&tmp, fs::Permissions::from_mode(m))?;
        }
        fs::rename(&tmp, &path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(|e| format!("{}: {e}", path.display()))
}

/// A JSON object file; `Ok(None)` when it does not exist.
pub fn read_json(path: &Path) -> Result<Option<Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let v: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: not JSON: {e}", path.display()))?;
    if !v.is_object() {
        return Err(format!("{}: not a JSON object", path.display()));
    }
    Ok(Some(v))
}

/// Sets `key` to `value` in an existing JSON object file, keeping the other
/// keys in their order; writes only on a change. Returns whether it wrote.
pub fn set_json_key(path: &Path, key: &str, value: &str) -> Result<bool, String> {
    let Some(mut v) = read_json(path)? else {
        return Err(format!("{}: missing", path.display()));
    };
    if v[key].as_str() == Some(value) {
        return Ok(false);
    }
    v[key] = Value::String(value.to_string());
    let mut text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    text.push('\n');
    write_atomic(path, &text)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_key_edit_keeps_order_mode_and_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        fs::write(&real, r#"{"z": 1, "theme": "dark", "a": {"b": [1, 2]}}"#).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(set_json_key(&link, "theme", "light").unwrap());
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let text = fs::read_to_string(&real).unwrap();
        let keys: Vec<_> = read_json(&real)
            .unwrap()
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys, ["z", "theme", "a"]);
        assert!(text.contains("\"light\""));
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Idempotent: no second write.
        assert!(!set_json_key(&link, "theme", "light").unwrap());
    }

    #[test]
    fn missing_and_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.json");
        assert_eq!(read_json(&p).unwrap(), None);
        assert!(set_json_key(&p, "theme", "dark").is_err());
        fs::write(&p, "not json").unwrap();
        assert!(read_json(&p).is_err());
        fs::write(&p, "[1]").unwrap();
        assert!(read_json(&p).is_err());
    }
}
