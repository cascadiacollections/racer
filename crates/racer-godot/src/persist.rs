//! Crash-safe text files.
//!
//! A save writes a temporary sibling and renames it over the real file, so a
//! crash mid-write leaves the previous file intact. A crash in the gap after
//! the temporary file is written and before the rename leaves only that
//! copy: readers fall back to it when the real file is not there. Layouts
//! already did this; ghosts, the daily record and run files did not, so the
//! first save of each could vanish.

use std::path::Path;

/// Read `path`, or its temporary sibling when the real file is missing.
pub fn read_replaced(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .or_else(|| std::fs::read_to_string(path.with_extension("tmp")).ok())
}

/// Write `text` to a temporary file beside `path`, then rename it over
/// `path`. A crash mid-write leaves the last good file.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&tmp, text))
        .and_then(|()| std::fs::rename(&tmp, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("racer-persist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_is_read_from_the_temporary_copy() {
        let dir = scratch("read");
        let path = dir.join("ghosts.txt");
        assert_eq!(read_replaced(&path), None);
        std::fs::write(path.with_extension("tmp"), "from the crash window").unwrap();
        assert_eq!(
            read_replaced(&path).as_deref(),
            Some("from the crash window")
        );
        std::fs::write(&path, "committed").unwrap();
        assert_eq!(read_replaced(&path).as_deref(), Some("committed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temporary() {
        let dir = scratch("write");
        let path = dir.join("daily.txt");
        write_atomic(&path, "first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        assert!(!path.with_extension("tmp").exists());
        write_atomic(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert!(!path.with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
