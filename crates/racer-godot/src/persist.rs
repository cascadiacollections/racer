//! Crash-safe text files, and a place to put one the game will not trust.
//!
//! A save writes a temporary sibling and renames it over the real file, so a
//! crash mid-write leaves the previous file. A crash in the gap after that
//! write and before the rename leaves the temporary copy: [`read_replaced`]
//! reads it when the real file is missing. For a run, that copy is also
//! adopted as the `.txt` ([`keep_aside_damaged_runs`]), because nothing loads
//! a run back — `just pull-runs` only sees `.txt` files.
//!
//! A file that is present but not a record used to be treated as empty, and
//! the next save wrote over the only copy. That included a file that is not
//! valid UTF-8: `read_to_string` fails, which looked the same as missing.
//! [`keep_aside`] renames it to a `.corrupt` sidecar (or `.corrupt-2`,
//! `.corrupt-3`, …) and never overwrites a sidecar. The original path is
//! then free for a new record. [`opened_save`] tells that file from one that
//! is simply absent, and does not substitute the temporary sibling for it.

use std::io;
use std::path::{Path, PathBuf};

/// What [`opened_save`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// Neither the file nor its temporary sibling is there.
    Missing,
    /// Text from `path` (the real file, or the temporary sibling when the
    /// real file is absent).
    Text {
        /// The file the text was read from.
        path: PathBuf,
        /// Its contents.
        text: String,
    },
    /// `path` is there, but it is not UTF-8 text. The temporary sibling was
    /// not read in its place.
    Unreadable {
        /// The file that could not be read as text.
        path: PathBuf,
    },
}

/// Read `path`, or its temporary sibling when the real file is missing.
///
/// `None` when the file is missing or present but not text. An unreadable
/// file is not treated as missing: use [`opened_save`] to tell them apart,
/// and do not replace it.
pub fn read_replaced(path: &Path) -> Option<String> {
    match opened_save(path) {
        Opened::Text { text, .. } => Some(text),
        Opened::Missing | Opened::Unreadable { .. } => None,
    }
}

/// The save at `path`: its text, or the temporary sibling's when the real
/// file is absent. A file that exists but is not UTF-8 is
/// [`Opened::Unreadable`], not missing, and hides the temporary sibling.
pub fn opened_save(path: &Path) -> Opened {
    match read_utf8(path) {
        Read::Missing => {}
        Read::Text(text) => {
            return Opened::Text {
                path: path.to_path_buf(),
                text,
            };
        }
        Read::Unreadable => {
            return Opened::Unreadable {
                path: path.to_path_buf(),
            }
        }
    }
    let tmp = path.with_extension("tmp");
    match read_utf8(&tmp) {
        Read::Missing => Opened::Missing,
        Read::Text(text) => Opened::Text { path: tmp, text },
        Read::Unreadable => Opened::Unreadable { path: tmp },
    }
}

/// Bytes at `path`, as text or not.
enum Read {
    Missing,
    Text(String),
    Unreadable,
}

/// Read `path` as UTF-8. A file that is there but is not text, or that
/// cannot be read, is [`Read::Unreadable`] so a caller cannot mistake it
/// for a missing file.
fn read_utf8(path: &Path) -> Read {
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Read::Text(text),
            Err(_) => Read::Unreadable,
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Read::Missing,
        Err(_) => {
            if path.is_file() {
                Read::Unreadable
            } else {
                Read::Missing
            }
        }
    }
}

/// Whether `path` is a run file ([`racer_sim::replay::decode_run`]).
/// Anything that is not UTF-8, or does not parse, is not.
pub fn is_run_file(path: &Path) -> bool {
    match read_utf8(path) {
        Read::Text(text) => racer_sim::replay::decode_run(&text).is_ok(),
        Read::Missing | Read::Unreadable => false,
    }
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

/// Run files in `dir` that the game should not leave as they are.
///
/// A `.txt` file that is not a run — including one that is not text — is
/// renamed aside. A `.tmp` sibling that *is* a run is the write that never
/// got renamed: it becomes the `.txt` (replacing one that is already there,
/// which is what the rename would have done). A `.tmp` that is not a run is
/// renamed aside, so the next save can use that name without eating the
/// bytes. Returns how many files were kept aside, not how many temporary
/// copies were adopted. A missing directory is nothing to do.
pub fn keep_aside_damaged_runs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    let mut n = 0;
    for path in &paths {
        if path.extension().is_some_and(|e| e == "txt")
            && !is_run_file(path)
            && keep_aside(path).is_ok()
        {
            n += 1;
        }
    }
    for path in &paths {
        if !path.extension().is_some_and(|e| e == "tmp") {
            continue;
        }
        if is_run_file(path) {
            // The rename the crash skipped. Replaces a `.txt` that is
            // already there, as the save itself would have. If it cannot,
            // keep the bytes aside instead of leaving them where the next
            // save's temporary file will overwrite them.
            if std::fs::rename(path, path.with_extension("txt")).is_err() {
                n += usize::from(keep_aside(path).is_ok());
            }
        } else if keep_aside(path).is_ok() {
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
        assert_eq!(opened_save(&path), Opened::Missing);
        std::fs::write(path.with_extension("tmp"), "from the crash window").unwrap();
        assert_eq!(
            read_replaced(&path).as_deref(),
            Some("from the crash window")
        );
        assert_eq!(
            opened_save(&path),
            Opened::Text {
                path: path.with_extension("tmp"),
                text: "from the crash window".into(),
            }
        );
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

    #[test]
    fn an_unreadable_file_is_not_missing_and_does_not_reveal_the_temporary() {
        let dir = scratch("binary");
        let path = dir.join("ghosts.txt");
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();
        std::fs::write(path.with_extension("tmp"), "the last good copy").unwrap();
        assert_eq!(read_replaced(&path), None);
        assert_eq!(
            opened_save(&path),
            Opened::Unreadable { path: path.clone() }
        );
        let aside = keep_aside(&path).unwrap();
        assert_eq!(std::fs::read(&aside).unwrap(), [0xff, 0xfe, 0xfd]);
        assert_eq!(read_replaced(&path).as_deref(), Some("the last good copy"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_finished_run_left_in_the_temporary_file_becomes_the_run() {
        let dir = scratch("tmp-run");
        let run = "racer-run 1\nepoch 1\nending won\nseed 0x0000000000000002\nlog v1\n";
        let older = "racer-run 1\nepoch 1\nending lost\nseed 0x0000000000000001\nlog v1\n";
        std::fs::write(dir.join("run-new.tmp"), run).unwrap();
        std::fs::write(dir.join("run-old.txt"), older).unwrap();
        std::fs::write(dir.join("run-old.tmp"), run).unwrap();
        std::fs::write(dir.join("run-junk.tmp"), [0xff, 0x00]).unwrap();
        std::fs::write(dir.join("run-bin.txt"), [0xff, 0x00]).unwrap();
        assert_eq!(keep_aside_damaged_runs(&dir), 2);
        assert_eq!(
            std::fs::read_to_string(dir.join("run-new.txt")).unwrap(),
            run
        );
        assert!(!dir.join("run-new.tmp").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("run-old.txt")).unwrap(),
            run
        );
        assert!(!dir.join("run-old.tmp").exists());
        assert_eq!(
            std::fs::read(dir.join("run-junk.tmp.corrupt")).unwrap(),
            [0xff, 0x00]
        );
        assert_eq!(
            std::fs::read(dir.join("run-bin.txt.corrupt")).unwrap(),
            [0xff, 0x00]
        );
        assert!(is_run_file(&dir.join("run-new.txt")));
        assert!(!is_run_file(&dir.join("run-bin.txt.corrupt")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
