//! Incremental reads of JSONL transcripts. Each file is read from the last complete line
//! consumed, so an append costs one short read. Files not written for 8 days are skipped.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::time::{now_ms, DAY_MS};

const STALE_MS: i64 = 8 * DAY_MS;

#[derive(Default)]
pub struct Tail {
    offsets: HashMap<PathBuf, u64>,
}

impl Tail {
    /// Calls `ingest(path, line)` for every complete new line of the `.jsonl` files under `roots`.
    pub fn scan(&mut self, roots: &[PathBuf], mut ingest: impl FnMut(&Path, &[u8])) {
        let cutoff = now_ms() - STALE_MS;
        let mut files = Vec::new();
        for root in roots {
            jsonl_files(root, &mut files);
        }
        for path in files {
            self.read_file(&path, cutoff, &mut ingest);
        }
    }

    fn read_file(&mut self, path: &Path, cutoff: i64, ingest: &mut impl FnMut(&Path, &[u8])) {
        let Ok(meta) = fs::metadata(path) else { return };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        if modified < cutoff {
            return;
        }
        let size = meta.len();
        let mut start = self.offsets.get(path).copied().unwrap_or(0);
        if size < start {
            start = 0; // rewritten shorter: read it again from the top
        }
        if size == start {
            return;
        }
        let Ok(mut file) = fs::File::open(path) else {
            return;
        };
        if file.seek(SeekFrom::Start(start)).is_err() {
            return;
        }
        let mut data = Vec::new();
        if file.read_to_end(&mut data).is_err() {
            return;
        }
        // Only consume complete lines; a partial last line is read once it is finished.
        let end = data.iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1);
        self.offsets.insert(path.to_path_buf(), start + end as u64);
        for line in data[..end].split(|&c| c == b'\n') {
            if !line.is_empty() {
                ingest(path, line);
            }
        }
    }
}

/// Every `*.jsonl` file under `dir`, recursively.
fn jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            jsonl_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// A fresh empty directory under the system temp dir, for tests.
#[cfg(test)]
pub fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("deskhub-ai-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(path: &Path, text: &str) {
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn lines(tail: &mut Tail, roots: &[PathBuf]) -> Vec<String> {
        let mut out = Vec::new();
        tail.scan(roots, |_, line| {
            out.push(String::from_utf8_lossy(line).into_owned())
        });
        out
    }

    #[test]
    fn reads_only_new_complete_lines() {
        let dir = temp_dir("tail");
        let file = dir.join("proj").join("s.jsonl");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(dir.join("notes.txt"), "ignored\n").unwrap();
        fs::write(&file, "one\ntwo\npart").unwrap();

        let mut tail = Tail::default();
        let roots = [dir.clone()];
        // The partial last line waits until its newline arrives.
        assert_eq!(lines(&mut tail, &roots), vec!["one", "two"]);
        append(&file, "ial\nthree\n");
        assert_eq!(lines(&mut tail, &roots), vec!["partial", "three"]);
        assert!(lines(&mut tail, &roots).is_empty());

        // A file that shrank is read again from the start.
        fs::write(&file, "new\n").unwrap();
        assert_eq!(lines(&mut tail, &roots), vec!["new"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
