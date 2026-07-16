use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct FileCache<T> {
    entries: HashMap<PathBuf, (SystemTime, Vec<T>)>,
}

impl<T: Clone> FileCache<T> {
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// Returns cached parsed items for `path` if its mtime hasn't changed
    /// since the last call; otherwise reads the file, calls `parse` on its
    /// content, caches the result, and returns it.
    pub fn get_or_parse(&mut self, path: &Path, parse: impl FnOnce(&str) -> Vec<T>) -> Vec<T> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some(mtime) = mtime {
            if let Some((cached_mtime, cached)) = self.entries.get(path) {
                if mtime == *cached_mtime {
                    return cached.clone();
                }
            }
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let parsed = parse(&content);
        if let Some(mtime) = mtime {
            self.entries.insert(path.to_path_buf(), (mtime, parsed.clone()));
        }
        parsed
    }
}

impl<T: Clone> Default for FileCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Write;

    #[test]
    fn reparses_only_when_mtime_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "v1").unwrap();

        let calls = Cell::new(0);
        let mut cache: FileCache<String> = FileCache::new();

        let result1 = cache.get_or_parse(&path, |content| {
            calls.set(calls.get() + 1);
            vec![content.to_string()]
        });
        assert_eq!(result1, vec!["v1".to_string()]);
        assert_eq!(calls.get(), 1);

        // Same content, same mtime -> no reparse.
        let result2 = cache.get_or_parse(&path, |content| {
            calls.set(calls.get() + 1);
            vec![content.to_string()]
        });
        assert_eq!(result2, vec!["v1".to_string()]);
        assert_eq!(calls.get(), 1);

        // Change content and bump mtime explicitly (avoids filesystem mtime
        // resolution flakiness).
        {
            let mut f = std::fs::OpenOptions::new().write(true).truncate(true).open(&path).unwrap();
            f.write_all(b"v2").unwrap();
        }
        let new_mtime = filetime::FileTime::from_unix_time(
            filetime::FileTime::now().unix_seconds() + 5,
            0,
        );
        filetime::set_file_mtime(&path, new_mtime).unwrap();

        let result3 = cache.get_or_parse(&path, |content| {
            calls.set(calls.get() + 1);
            vec![content.to_string()]
        });
        assert_eq!(result3, vec!["v2".to_string()]);
        assert_eq!(calls.get(), 2);
    }
}
