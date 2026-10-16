//! Size-bounded service logs shared by the CLI and daemons.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_FILES: usize = 7;

/// An append-only writer retaining 20 MiB per file and seven files per service.
/// A separate lock file coordinates rotation across concurrent CLI processes.
/// Files are reopened under that lock so no process writes to a renamed archive.
pub struct LogWriter {
    dir: PathBuf,
    prefix: String,
    lock: File,
    max_bytes: u64,
}

impl LogWriter {
    pub fn new(dir: &Path, prefix: &str) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let writer = Self {
            dir: dir.to_owned(),
            prefix: prefix.to_owned(),
            lock: OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(dir.join(format!("{prefix}.lock")))?,
            max_bytes: MAX_FILE_BYTES,
        };
        writer.locked(|| writer.prune())?;
        Ok(writer)
    }

    fn write_locked(&self, buf: &[u8]) -> io::Result<usize> {
        let mut remaining = buf;
        while !remaining.is_empty() {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path(0))?;
            let size = file.metadata()?.len();
            // Rotate before the next record where possible, preserving JSON lines.
            if size > 0 && size.saturating_add(remaining.len() as u64) > self.max_bytes {
                drop(file);
                self.rotate()?;
                file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.path(0))?;
            }
            let length = remaining.len().min(self.max_bytes as usize);
            file.write_all(&remaining[..length])?;
            remaining = &remaining[length..];
        }
        Ok(buf.len())
    }

    fn path(&self, index: usize) -> PathBuf {
        self.dir.join(if index == 0 {
            self.prefix.clone()
        } else {
            format!("{}.{index}", self.prefix)
        })
    }

    fn locked<T>(&self, action: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        self.lock.lock()?;
        let result = action();
        let unlocked = self.lock.unlock();
        match result {
            Ok(value) => unlocked.map(|()| value),
            Err(error) => Err(error),
        }
    }

    fn rotate(&self) -> io::Result<()> {
        for index in (1..MAX_FILES).rev() {
            let source = self.path(index - 1);
            match fs::rename(source, self.path(index)) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        self.prune()
    }

    // Include legacy daily files in the same budget, but never unrelated files,
    // directories, or symlinks. Run on startup as well as every size rotation.
    fn prune(&self) -> io::Result<()> {
        let mut files = Vec::new();
        let mut total = 0;
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let managed = name == self.prefix
                || name
                    .strip_prefix(&format!("{}.", self.prefix))
                    .is_some_and(is_log_suffix);
            if !managed {
                continue;
            }
            let metadata = entry.metadata()?;
            total += metadata.len();
            files.push((entry.path(), metadata.modified()?, metadata.len()));
        }
        // Reserve a full active file even before it exists or fills up.
        let active_size = files
            .iter()
            .find(|(path, _, _)| *path == self.path(0))
            .map(|(_, _, size)| *size);
        total += self.max_bytes.saturating_sub(active_size.unwrap_or(0));
        files.sort_by_key(|(_, modified, _)| *modified);
        let mut count = files.len() + usize::from(active_size.is_none());
        for (path, _, size) in files {
            if count <= MAX_FILES && total <= self.max_bytes * MAX_FILES as u64 {
                break;
            }
            if path == self.path(0) {
                continue;
            }
            fs::remove_file(path)?;
            count -= 1;
            total -= size;
        }
        Ok(())
    }
}

fn is_log_suffix(suffix: &str) -> bool {
    if suffix
        .parse::<usize>()
        .is_ok_and(|n| (1..MAX_FILES).contains(&n))
    {
        return true;
    }
    // Previous daily appender: YYYY-MM-DD.
    suffix.len() == 10
        && suffix.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.locked(|| self.write_locked(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn bounds_size_and_retains_newest_records() {
        let dir = TempDir::new().unwrap();
        let mut writer = LogWriter::new(dir.path(), "server.log").unwrap();
        writer.max_bytes = 8;
        for i in 0..20 {
            writeln!(writer, "{i:07}").unwrap();
        }
        assert_eq!(fs::read_to_string(writer.path(0)).unwrap(), "0000019\n");
        assert_eq!(fs::read_to_string(writer.path(6)).unwrap(), "0000013\n");
        assert!(!writer.path(7).exists());
        for i in 0..7 {
            assert!(fs::metadata(writer.path(i)).unwrap().len() <= 8);
        }
    }

    #[test]
    fn concurrent_writers_reopen_after_rotation() {
        let dir = TempDir::new().unwrap();
        let mut first = LogWriter::new(dir.path(), "ur.log").unwrap();
        let mut second = LogWriter::new(dir.path(), "ur.log").unwrap();
        first.max_bytes = 4;
        second.max_bytes = 4;
        first.write_all(b"one\n").unwrap();
        second.write_all(b"two\n").unwrap();
        first.write_all(b"end\n").unwrap();
        assert_eq!(fs::read(first.path(0)).unwrap(), b"end\n");
        assert_eq!(fs::read(first.path(1)).unwrap(), b"two\n");
        assert_eq!(fs::read(first.path(2)).unwrap(), b"one\n");
    }

    #[test]
    fn startup_prunes_legacy_files_and_preserves_unrelated_entries() {
        let dir = TempDir::new().unwrap();
        for day in 10..20 {
            fs::write(dir.path().join(format!("server.log.2026-03-{day}")), b"old").unwrap();
        }
        for name in [
            "server.log.lock",
            "server.log.backup",
            "builderd.log.2026-03-10",
        ] {
            fs::write(dir.path().join(name), b"keep").unwrap();
        }
        fs::create_dir(dir.path().join("server.log.2026-03-01")).unwrap();
        let _writer = LogWriter::new(dir.path(), "server.log").unwrap();
        let dated = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_type().unwrap().is_file()
                    && e.file_name()
                        .to_string_lossy()
                        .starts_with("server.log.2026")
            })
            .count();
        assert_eq!(dated, 6);
        assert!(dir.path().join("server.log.backup").exists());
        assert!(dir.path().join("builderd.log.2026-03-10").exists());
        assert!(dir.path().join("server.log.2026-03-01").is_dir());
    }

    #[test]
    fn legacy_bytes_leave_room_for_active_file() {
        let dir = TempDir::new().unwrap();
        for day in 10..20 {
            fs::write(
                dir.path().join(format!("server.log.2026-03-{day}")),
                [0; 10],
            )
            .unwrap();
        }
        let mut writer = LogWriter::new(dir.path(), "server.log").unwrap();
        writer.max_bytes = 4;
        writer.locked(|| writer.prune()).unwrap();
        writer.write_all(b"live").unwrap();
        let sizes: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name() != "server.log.lock")
            .map(|entry| entry.metadata().unwrap().len())
            .collect();
        assert!(sizes.len() <= MAX_FILES);
        assert!(sizes.iter().sum::<u64>() <= 4 * MAX_FILES as u64);
    }

    #[test]
    fn oversized_writes_remain_bounded() {
        let dir = TempDir::new().unwrap();
        let mut writer = LogWriter::new(dir.path(), "test.log").unwrap();
        writer.max_bytes = 4;
        writer.write_all(&[b'x'; 100]).unwrap();
        for i in 0..7 {
            assert_eq!(fs::metadata(writer.path(i)).unwrap().len(), 4);
        }
    }
}
