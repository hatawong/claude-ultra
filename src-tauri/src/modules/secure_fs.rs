//! Secure file I/O helpers.
//!
//! All credential-bearing files (accounts, config, ~/.claude/settings.json) MUST use
//! [`secure_write`] to guarantee atomic replacement + owner-only permissions.
//!
//! Without these guarantees:
//! - A crash mid-write leaves the file in a half-written state, corrupting credentials
//!   that take effort (or external re-auth) to restore.
//! - Default umask (022 on many macOS configs) leaves files world-readable, leaking
//!   `session_key` / `access_token` / `refresh_token` to any local process.

use std::path::Path;

/// Write `content` to `path` atomically with owner-only permissions (0600 on Unix).
///
/// Implementation rationale (v1.0.1 hardening — address fix-review findings):
///
/// 1. **No permission window**: on Unix the temp file is created with `mode(0o600)`
///    via `OpenOptions` at creation time, not chmod'd after. A naive `fs::write`
///    would first create at the umask-derived mode (typically 0644), giving a
///    brief window where other local processes could read the credential bytes.
/// 2. **chmod failure is not silent**: if mode enforcement on Unix fails
///    (e.g. `create_new` hit a dangling tmp, FS doesn't support permissions),
///    we return an error instead of continuing with unenforced permissions.
/// 3. **Per-write unique tmp name**: appending a fresh UUID to the tmp filename
///    prevents concurrent writers of the same target from clobbering each
///    other's in-flight temp files.
/// 4. **Cleanup on failure**: if any step after tmp creation fails, we best-effort
///    remove the tmp file so repeated crashes don't leave litter on disk.
///
/// The original `{path}` is never left in a half-written state because the final
/// step is a rename (atomic on same filesystem).
pub fn secure_write(path: &Path, content: &[u8]) -> Result<(), String> {
    // Ensure parent directory exists (callers may skip this for fresh installs)
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create dir {}: {}", parent.display(), e))?;
        }
    }

    // Unique tmp path sibling to target: same filesystem → rename is atomic.
    // UUID suffix prevents concurrent writers from racing on the same tmp name.
    let tmp_suffix = uuid::Uuid::new_v4().simple().to_string();
    let tmp_path = match path.extension() {
        Some(ext) => path.with_extension(format!("{}.{}.tmp", ext.to_string_lossy(), tmp_suffix)),
        None => path.with_extension(format!("{}.tmp", tmp_suffix)),
    };

    // Create + write with strict mode at creation time (Unix).
    // Windows: ACLs inherit from parent; no world-readable-by-default problem.
    let write_result: Result<(), String> = (|| {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true) // fail if tmp already exists (shouldn't, due to UUID)
                .mode(0o600)       // created with 0600 — no umask window
                .open(&tmp_path)
                .map_err(|e| format!("Failed to create {}: {}", tmp_path.display(), e))?;
            f.write_all(content)
                .map_err(|e| format!("Failed to write {}: {}", tmp_path.display(), e))?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            // On Windows, ACLs are inherited and files are not world-readable by default.
            std::fs::write(&tmp_path, content)
                .map_err(|e| format!("Failed to write {}: {}", tmp_path.display(), e))
        }
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Atomic replace. On Windows rename-to-existing fails; use remove + rename as fallback.
    #[cfg(unix)]
    let rename_result = std::fs::rename(&tmp_path, path);
    #[cfg(not(unix))]
    let rename_result = {
        // Windows: remove existing target first (still non-atomic, accepted for v1.0.1).
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp_path, path)
    };

    if let Err(e) = rename_result {
        let msg = format!(
            "Failed to rename {} → {}: {}",
            tmp_path.display(),
            path.display(),
            e
        );
        let _ = std::fs::remove_file(&tmp_path);
        return Err(msg);
    }

    Ok(())
}

/// Convenience wrapper: serialize a type to pretty JSON and write securely.
pub fn secure_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let content = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize to JSON: {}", e))?;
    secure_write(path, content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "claude_ultra_secure_fs_test_{}_{}",
            name,
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn test_secure_write_creates_file_with_content() {
        let path = tmp_path("basic");
        secure_write(&path, b"hello world").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello world");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_secure_write_overwrites_existing() {
        let path = tmp_path("overwrite");
        fs::write(&path, b"old").unwrap();
        secure_write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        let _ = fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn test_secure_write_sets_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp_path("perms");
        secure_write(&path, b"secret").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file must be owner-only readable/writable");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_secure_write_creates_parent_dir() {
        let dir = tmp_path("parent_dir");
        let path = dir.join("nested").join("file.json");
        secure_write(&path, b"{}").unwrap();
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_secure_write_json_serializes_and_writes() {
        let path = tmp_path("json");
        let value = serde_json::json!({"key": "value", "n": 42});
        secure_write_json(&path, &value).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed, value);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_secure_write_no_temp_file_left_on_success() {
        let path = tmp_path("no_temp");
        secure_write(&path, b"ok").unwrap();
        // With UUID-suffixed tmp names we can't predict the exact tmp path,
        // but we can assert the parent directory contains only our target.
        let parent = path.parent().unwrap();
        let leaked: Vec<_> = fs::read_dir(parent).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with(&path.file_name().unwrap().to_string_lossy().to_string())
                    && name.contains(".tmp")
            })
            .collect();
        assert!(leaked.is_empty(), "no .tmp files must remain after success, found: {:?}",
                leaked.iter().map(|e| e.file_name()).collect::<Vec<_>>());
        let _ = fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn test_secure_write_no_umask_window() {
        // Regression test for v1.0.1 fix-review P1: prior implementation used
        // fs::write() + chmod, creating a window where the file existed at
        // umask-derived mode (0644) with credentials inside before chmod ran.
        // Current impl uses OpenOptions::mode(0o600).create_new(true), which sets
        // 0600 atomically at creation.
        //
        // We can't directly observe the tmp file's mode during the write (it's
        // renamed away immediately), but we can verify the final file is 0600
        // AND that the tmp-creation code path uses mode() by inspecting source
        // — this test just guards the end-state invariant.
        use std::os::unix::fs::PermissionsExt;
        let path = tmp_path("no_window");
        secure_write(&path, b"credentials").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "final file must be 0600");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_secure_write_concurrent_same_target_no_race() {
        // UUID-suffixed tmp names prevent two concurrent writers from clobbering
        // each other's in-flight temp files. Last writer wins on the final rename.
        use std::sync::Arc;
        use std::thread;
        let path = Arc::new(tmp_path("concurrent"));
        let mut handles = vec![];
        for i in 0..8 {
            let p = Arc::clone(&path);
            handles.push(thread::spawn(move || {
                secure_write(&p, format!("writer-{}", i).as_bytes()).unwrap();
            }));
        }
        for h in handles { h.join().unwrap(); }
        // File exists and contains some valid writer's content
        let content = fs::read_to_string(&*path).unwrap();
        assert!(content.starts_with("writer-"), "got: {}", content);
        let _ = fs::remove_file(&*path);
    }
}
