//! Built tracks kept on disk, so relaunching or racing a seed again never
//! builds a track twice (DESIGN §6, track store).
//!
//! One small text file per track in `user://tracks`, named by its key:
//!
//! ```text
//! racer-track 1                          format (TRACK_FILE)
//! key 0x00000000000000ff 2 1             run seed, index, branch
//! epoch 1 0x7038998bdfbbe7ab             SIM_EPOCH and its digest
//! digest 0x…                             track_hash of the track
//! allot 1234 1310 …                      the timers, ticks
//! code RD…                               the road: a draft layout code
//! ```
//!
//! The road is written as a layout code (DESIGN §9.5), which holds exactly
//! the integers a track is made of; the timers ride beside it. Reading it
//! back rebuilds the track ([`Layout::track`], then the timers) and keeps
//! it only if its [`track_hash`] is the digest written, so a damaged,
//! truncated or foreign file is a miss, never a wrong track. A file from
//! another epoch (the generator may have moved: its digest is the
//! "generator version") or another format is a miss too, and is replaced
//! when the track is built again.
//!
//! Writes go to a temporary file renamed into place, so a crash leaves the
//! old file or the new one. The directory is held to [`CAPACITY`] files:
//! past it the least recently used go (a read refreshes a file's time).
//! Every failure is a miss or a skipped write: the store can only make a
//! track arrive sooner, never change it. It runs on the builder threads and
//! uses only the standard library, never the engine.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use racer_sim::layout::Layout;
use racer_sim::track::{track_hash, Track};

use crate::builder::{Key, Source};

/// Version of the file, its first line's second token.
pub const TRACK_FILE: u32 = 1;

/// The most tracks kept: about a week of daily runs (9 tracks each) and
/// the odd other seed.
pub const CAPACITY: usize = 64;

/// The file's extension.
const EXT: &str = "trk";

/// Tracks on disk in one directory.
#[derive(Clone, Debug)]
pub struct TrackStore {
    dir: PathBuf,
    capacity: usize,
}

/// The text of `track` as key `key`'s file, or `None` if its road cannot be
/// written as a layout code.
pub fn encode(key: Key, track: &Track) -> Option<String> {
    let code = Layout::from_track(track).to_code().ok()?;
    let allot: Vec<String> = track.allot_ticks().iter().map(u32::to_string).collect();
    Some(format!(
        "racer-track {TRACK_FILE}\nkey {:#018x} {} {}\nepoch {} {:#018x}\ndigest {:#018x}\nallot {}\ncode {code}\n",
        key.0,
        key.1,
        key.2,
        racer_sim::SIM_EPOCH,
        racer_sim::EPOCH_DIGEST,
        track_hash(track),
        allot.join(" "),
    ))
}

/// The track in `text`, if it is key `key`'s file from this format and
/// epoch and the track it rebuilds has the digest it names.
pub fn decode(key: Key, text: &str) -> Option<Track> {
    let mut lines = text.lines();
    let mut field = |name: &str| {
        lines
            .next()
            .and_then(|l| l.strip_prefix(name))
            .and_then(|l| l.strip_prefix(' '))
    };
    if field("racer-track")? != TRACK_FILE.to_string() {
        return None;
    }
    let want_key = format!("{:#018x} {} {}", key.0, key.1, key.2);
    if field("key")? != want_key {
        return None;
    }
    let want_epoch = format!("{} {:#018x}", racer_sim::SIM_EPOCH, racer_sim::EPOCH_DIGEST);
    if field("epoch")? != want_epoch {
        return None;
    }
    let digest = u64::from_str_radix(field("digest")?.strip_prefix("0x")?, 16).ok()?;
    let allot = field("allot")?
        .split_whitespace()
        .map(|a| a.parse::<u32>().ok())
        .collect::<Option<Vec<u32>>>()?;
    let layout = Layout::from_code(field("code")?).ok()?;
    // Empty or zero allotments are not a calibration. Loading one would
    // hand the game a track the prover did not pass.
    if layout.proof.is_some()
        || allot.is_empty()
        || allot.len() != layout.checkpoints.len()
        || allot.contains(&0)
    {
        return None;
    }
    let mut track = layout.track().ok()?;
    track.set_allotments(allot);
    (track_hash(&track) == digest).then_some(track)
}

impl TrackStore {
    /// A store in `dir` (made on the first write) holding [`CAPACITY`]
    /// tracks.
    pub fn new(dir: impl Into<PathBuf>) -> TrackStore {
        TrackStore::with_capacity(dir, CAPACITY)
    }

    /// A store holding `capacity` tracks (at least one).
    pub fn with_capacity(dir: impl Into<PathBuf>, capacity: usize) -> TrackStore {
        TrackStore {
            dir: dir.into(),
            capacity: capacity.max(1),
        }
    }

    /// Where it keeps its files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Key `key`'s file.
    pub fn path(&self, key: Key) -> PathBuf {
        self.dir
            .join(format!("{:016x}-{}-{}.{EXT}", key.0, key.1, key.2))
    }

    /// The stored track for `key`, if there is a good one. A hit is marked
    /// as just used.
    pub fn load(&self, key: Key) -> Option<Track> {
        let path = self.path(key);
        let track = decode(key, &std::fs::read_to_string(&path).ok()?)?;
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(track)
    }

    /// Keep `track` as `key`'s file, then trim the store to its capacity.
    /// Returns whether it was written.
    pub fn save(&self, key: Key, track: &Track) -> bool {
        let Some(text) = encode(key, track) else {
            return false;
        };
        let path = self.path(key);
        // Two workers never write one key at once (each key is built
        // once), but a name of their own costs nothing.
        let tmp = path.with_extension(format!("{:?}.tmp", std::thread::current().id()));
        let written = std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(&tmp, text))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return false;
        }
        self.trim();
        true
    }

    /// Delete the least recently used files past the capacity, and any
    /// temporary file a crash left.
    fn trim(&self) {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
        for e in dir.flatten() {
            let path = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            let age = e.metadata().and_then(|m| m.modified()).ok();
            if name.ends_with(".tmp") {
                // Another worker's write in progress is seconds old at most.
                let stale = age
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|d| d.as_secs() > 60);
                if stale {
                    let _ = std::fs::remove_file(&path);
                }
            } else if path.extension().is_some_and(|x| x == EXT) {
                files.push((age.unwrap_or(SystemTime::UNIX_EPOCH), path));
            }
        }
        if files.len() <= self.capacity {
            return;
        }
        files.sort();
        let excess = files.len() - self.capacity;
        for (_, path) in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// A [`Source`] that reads from this store and builds with `build` on
    /// a miss, keeping what it built.
    pub fn source(self, build: Source) -> Source {
        Arc::new(move |key: Key| match self.load(key) {
            Some(t) => Ok(Arc::new(t)),
            None => {
                let t = build(key)?;
                self.save(key, &t);
                Ok(t)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use racer_sim::run::{Generate, Tracks};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh, empty directory under the system's temporary directory.
    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("racer-track-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn generated_tracks_round_trip_exactly() {
        for key in [(1, 0, 0), (7, 2, 1), (0xdead_beef, 4, 0), (42, 3, 1)] {
            let built = Generate.track(key.0, key.1, key.2).expect("proves");
            let text = encode(key, &built).expect("a generated road is a layout");
            let back = decode(key, &text).expect("reads back");
            assert_eq!(back, *built, "{key:?}");
            assert_eq!(track_hash(&back), track_hash(&built));
        }
    }

    /// Every track of 40 runs: `cargo test --release -p racer-godot
    /// round_trip_survey -- --ignored`.
    #[test]
    #[ignore]
    fn round_trip_survey() {
        for seed in 1..=40u64 {
            for (i, b) in std::iter::once((0, 0)).chain((1..5).flat_map(|i| [(i, 0), (i, 1)])) {
                let key = (seed, i, b);
                let built = Generate.track(seed, i, b).expect("proves");
                let back = encode(key, &built).and_then(|t| decode(key, &t));
                assert_eq!(back.as_ref(), Some(&*built), "{key:?}");
            }
        }
    }

    #[test]
    fn an_uncalibrated_or_zero_timer_is_not_loaded() {
        let key = (1, 0, 0);
        let mut bare = racer_sim::track::fixtures::drag_strip();
        let empty = encode(key, &bare).unwrap();
        assert!(decode(key, &empty).is_none(), "no allotments");
        bare.set_allotments(vec![0]);
        let zeros = encode(key, &bare).unwrap();
        assert!(decode(key, &zeros).is_none(), "a zero timer");
        bare.set_allotments(vec![120]);
        let timed = encode(key, &bare).unwrap();
        assert_eq!(decode(key, &timed).as_ref(), Some(&bare));
    }

    #[test]
    fn a_store_keeps_a_track_and_a_second_source_never_builds_it() {
        let dir = scratch("source");
        let builds = Arc::new(AtomicUsize::new(0));
        let n = builds.clone();
        let build: Source = Arc::new(move |k: Key| {
            n.fetch_add(1, Ordering::SeqCst);
            Generate.track(k.0, k.1, k.2).map_err(|_| ())
        });
        let key = (3, 1, 0);
        let a = TrackStore::new(&dir).source(build.clone())(key).unwrap();
        // A relaunch: a new store on the same directory.
        let b = TrackStore::new(&dir).source(build)(key).unwrap();
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert_eq!(*a, *b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn damage_of_any_kind_is_a_miss_and_is_rebuilt() {
        let dir = scratch("damage");
        let s = TrackStore::new(&dir);
        let key = (5, 0, 0);
        let t = Generate.track(5, 0, 0).expect("proves");
        assert!(s.save(key, &t));
        let good = std::fs::read_to_string(s.path(key)).unwrap();
        let damaged = [
            String::new(),
            "garbage".into(),
            good[..good.len() / 2].to_string(),
            good.replace("racer-track 1", "racer-track 9"),
            good.replace("key 0x0000000000000005 0 0", "key 0x0000000000000006 0 0"),
            // An allotment changed: the code is fine, the digest says no.
            {
                let mut l: Vec<String> = good.lines().map(String::from).collect();
                l[4] = l[4].replacen(' ', " 1", 1);
                l.join("\n")
            },
            // A flipped character in the code: its checksum says no.
            {
                let at = good.find("code RD").unwrap() + 12;
                let mut b = good.clone().into_bytes();
                b[at] = if b[at] == b'7' { b'8' } else { b'7' };
                String::from_utf8(b).unwrap()
            },
        ];
        for (i, text) in damaged.iter().enumerate() {
            std::fs::write(s.path(key), text).unwrap();
            assert!(s.load(key).is_none(), "damage {i} read as a track");
        }
        // The source rebuilds it and writes it back whole.
        let back = s.clone().source(crate::builder::generate())(key).unwrap();
        assert_eq!(*back, *t);
        assert_eq!(s.load(key).as_ref(), Some(&*t));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_epoch_is_a_miss() {
        let key = (8, 1, 1);
        let t = Generate.track(8, 1, 1).expect("proves");
        let text = encode(key, &t).unwrap();
        let this = format!("epoch {} ", racer_sim::SIM_EPOCH);
        let other = text.replace(&this, &format!("epoch {} ", racer_sim::SIM_EPOCH + 1));
        assert_ne!(text, other);
        assert!(decode(key, &other).is_none());
        let digest = format!("{:#018x}\ndigest", racer_sim::EPOCH_DIGEST);
        let other = text.replace(&digest, "0x0000000000000001\ndigest");
        assert!(decode(key, &other).is_none(), "another generator digest");
        assert!(decode(key, &text).is_some());
    }

    #[test]
    fn the_store_keeps_only_the_most_recently_used() {
        let dir = scratch("evict");
        let s = TrackStore::with_capacity(&dir, 3);
        let mut t = racer_sim::track::fixtures::drag_strip();
        t.set_allotments(vec![120]);
        let at = |secs: u64| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        for seed in 1..=3u64 {
            assert!(s.save((seed, 0, 0), &t));
            let f = std::fs::File::options()
                .write(true)
                .open(s.path((seed, 0, 0)))
                .unwrap();
            f.set_modified(at(1_000_000 + seed)).unwrap();
        }
        // Reading the oldest makes it the newest.
        assert!(s.load((1, 0, 0)).is_some());
        // A leftover temporary file from a crash goes too.
        std::fs::write(dir.join("x.tmp"), "half").unwrap();
        std::fs::File::options()
            .write(true)
            .open(dir.join("x.tmp"))
            .unwrap()
            .set_modified(at(1))
            .unwrap();
        assert!(s.save((4, 0, 0), &t));
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "0000000000000001-0-0.trk",
                "0000000000000003-0-0.trk",
                "0000000000000004-0-0.trk"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_store_still_builds() {
        let file = scratch("blocked");
        std::fs::write(&file, "not a directory").unwrap();
        let s = TrackStore::new(&file);
        let t = s.clone().source(crate::builder::generate())((2, 0, 0)).unwrap();
        assert_eq!(*t, *Generate.track(2, 0, 0).expect("proves"));
        assert!(s.load((2, 0, 0)).is_none());
        let _ = std::fs::remove_file(&file);
    }
}
