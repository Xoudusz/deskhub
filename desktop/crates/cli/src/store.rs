//! Home directory lookup and atomic file writes shared by both modes.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Home directory. `DESKHUB_HOME` overrides it (tests); otherwise USERPROFILE on Windows, HOME elsewhere.
pub fn home_dir() -> Option<PathBuf> {
    let from_env = std::env::var_os("DESKHUB_HOME").filter(|v| !v.is_empty());
    let home = from_env.or_else(|| {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        std::env::var_os(var).filter(|v| !v.is_empty())
    })?;
    Some(PathBuf::from(home))
}

/// `~/.deskhub`, the directory the companion reads from.
pub fn deskhub_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".deskhub"))
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Write to a temp file next to `path`, then rename it over `path`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("out");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = dir.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));
    let result = write_then_rename(&tmp, path, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_then_rename(tmp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(tmp)?;
    file.write_all(bytes)?;
    drop(file);
    fs::rename(tmp, path)
}

#[cfg(test)]
pub fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "deskhub-cli-{label}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_file_and_leaves_no_temp() {
        let dir = temp_dir("atomic");
        let path = dir.join("claude_status.json");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "leftover temp file: {names:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
