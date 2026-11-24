//! Crash-safe text files, and a place to put one the game will not trust.
//!
//! A save writes a temporary sibling and renames it over the real file, so a
//! crash mid-write leaves the previous file. A crash in the gap after that
//! write and before the rename leaves only the temporary copy: [`read_replaced`]
//! reads it when the real file is missing.
//!
//! A file that is present but not a record used to be treated as empty, and
//! the next save wrote over the only copy. [`keep_aside`] renames it to a
//! `.corrupt` sidecar (or `.corrupt-2`, `.corrupt-3`, …) and never overwrites
//! a sidecar. The original path is then free for a new record.

use std::io;
use std::path::{Path, PathBuf};

/// Read `path`, or its temporary sibling when the real file is missing.
pub fn read_replaced(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .or_else(|| std::fs::read_to_string(path.with_extension("tmp")).ok())
}

/// The file [`read_replaced`] would read: the real path when it exists, else
/// the temporary sibling. `None` when neither is there.
pub fn opened_save(path: &Path) -> Option<(PathBuf, String)> {
    if path.is_file() {
        return std::fs::read_to_string(path)
            .ok()
            .map(|text| (path.to_path_buf(), text));
    }
    let tmp = path.with_extension("tmp");
    std::fs::read_to_string(&tmp).ok().map(|text| (tmp, text))
}

/// Write `text` to a temporary sibling and rename it over `path`, so a crash
/// mid-write leaves the previous file.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&tmp, text))
        .and_then(|()| std::fs::rename(&tmp, path))
}

/// Rename `path` to a sidecar beside it. `path` itself is gone on success,
/// and no existing sidecar is replaced.
pub fn keep_aside(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let name = name.to_string_lossy();
    let mut dest = path.with_file_name(format!("{name}.corrupt"));
    let mut n = 2u32;
    while dest.exists() {
        dest = path.with_file_name(format!("{name}.corrupt-{n}"));
        n += 1;
        if n > 100 {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "too many corrupt sidecars",
            ));
        }
    }
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

/// Run files in `dir` that do not parse. Each is renamed aside. Returns how
/// many were kept aside. A missing directory is nothing to do.
pub fn keep_aside_damaged_runs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e == "txt") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if racer_sim::replay::decode_run(&text).is_ok() {
            continue;
        }
        if keep_aside(&path).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
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
        assert!(opened_save(&path).is_none());
        std::fs::write(path.with_extension("tmp"), "from the crash window").unwrap();
        assert_eq!(
            read_replaced(&path).as_deref(),
            Some("from the crash window")
        );
        let (got, text) = opened_save(&path).unwrap();
        assert_eq!(got, path.with_extension("tmp"));
        assert_eq!(text, "from the crash window");
        std::fs::write(&path, "committed").unwrap();
        assert_eq!(read_replaced(&path).as_deref(), Some("committed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_is_renamed_aside_and_not_overwritten() {
        let dir = scratch("one");
        let path = dir.join("daily.txt");
        std::fs::write(&path, "not a record").unwrap();
        let aside = keep_aside(&path).unwrap();
        assert_eq!(aside, dir.join("daily.txt.corrupt"));
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "not a record");
        std::fs::write(&path, "still bad").unwrap();
        let second = keep_aside(&path).unwrap();
        assert_eq!(second, dir.join("daily.txt.corrupt-2"));
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "not a record");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "still bad");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temporary() {
        let dir = scratch("write");
        let path = dir.join("runs").join("run.txt");
        write_atomic(&path, "first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        assert!(!path.with_extension("tmp").exists());
        write_atomic(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_run_is_kept_aside_and_a_good_one_stays() {
        let dir = scratch("runs");
        std::fs::write(
            dir.join("run-ok.txt"),
            "racer-run 1\nepoch 1\nending won\nseed 0x0000000000000001\nlog v1\n",
        )
        .unwrap();
        std::fs::write(dir.join("run-cut.txt"), "racer-run 1\nepoch").unwrap();
        std::fs::write(dir.join("notes.corrupt"), "leave me").unwrap();
        assert_eq!(keep_aside_damaged_runs(&dir), 1);
        assert!(dir.join("run-ok.txt").is_file());
        assert!(!dir.join("run-cut.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(dir.join("run-cut.txt.corrupt")).unwrap(),
            "racer-run 1\nepoch"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.corrupt")).unwrap(),
            "leave me"
        );
        assert_eq!(keep_aside_damaged_runs(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
