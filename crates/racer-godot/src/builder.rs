//! Building tracks and proving layouts off the main thread.
//!
//! A track is proven drivable before it is shown (DESIGN §2.4), which takes
//! 70 ms to 0.7 s on a desktop (2.7 s at worst) and more on a phone: far
//! too long for a frame. The game asks for both sides of the next fork as
//! a race starts, worker threads build them while the player drives, and
//! the fork screen waits only if the player is faster than the builders.
//!
//! There are two workers ([`WORKERS`], fewer on a one-core machine), so
//! the two sides of a fork are built side by side, and two queues: what
//! the game needs now ([`Build::request`], and every proof) is taken before
//! what it may need later ([`Build::prefetch`]: the daily run, built in the
//! background from launch). Asking for a prefetched key moves it to the
//! front.
//!
//! The editor's PROVE (and the check of a pasted proof) is the same kind of
//! work, tens to hundreds of milliseconds, so it goes to the same workers
//! ([`Build::prove`]), and its result comes back through [`Build::proofs`]
//! (DESIGN §9.11).
//!
//! Building is a pure function of `(run_seed, index, branch)`, and proving
//! of the layout, so where it happens, on which worker and in what order
//! cannot change a result. The workers only call into `racer-sim` and the
//! file system (`track_store`); they never touch the engine (gdext's
//! `lazy-function-tables` forbids engine calls off the main thread).

use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use racer_sim::layout::{Layout, ProveError};
use racer_sim::run::{Generate, Tracks};
use racer_sim::solver::Report;
use racer_sim::track::Track;

/// Which track: `(run_seed, index, branch)`.
pub type Key = (u64, u32, u8);

/// A proof's outcome: [`Layout::prove`]'s.
pub type Proved = Result<(Layout, Report), ProveError>;

/// Something that builds tracks and proves layouts on request and hands
/// them back later.
pub trait Build {
    /// Start building `key`, ahead of anything prefetched. Asking twice is
    /// harmless.
    fn request(&mut self, key: Key);
    /// Build `key` when nothing is requested: a track the game may want
    /// later. Asking twice, or for a key already requested, is harmless.
    fn prefetch(&mut self, key: Key) {
        self.request(key);
    }
    /// The tracks finished since the last poll.
    fn poll(&mut self) -> Vec<(Key, Arc<Track>)>;
    /// Start proving `layout` as job `job`.
    fn prove(&mut self, job: u64, layout: Layout);
    /// The proofs finished since the last call.
    fn proofs(&mut self) -> Vec<(u64, Proved)>;
}

/// The most worker threads: one per side of a fork.
pub const WORKERS: usize = 2;

/// How many workers to run here: [`WORKERS`], or one on a single core.
pub fn worker_count() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(WORKERS))
}

/// How a worker gets a track: built, or read back from the track store.
/// Must be a pure function of the key.
pub type Source = Arc<dyn Fn(Key) -> Arc<Track> + Send + Sync>;

/// [`Generate`], as a [`Source`].
pub fn generate() -> Source {
    Arc::new(|k: Key| Generate.track(k.0, k.1, k.2))
}

enum Job {
    Track(Key),
    Prove(u64, Layout),
}

enum Done {
    Track(Key, Arc<Track>),
    Proof(u64, Box<Proved>),
}

/// The work waiting for a worker.
#[derive(Default)]
struct Queues {
    /// Requests and proofs, in order: taken first.
    now: VecDeque<Job>,
    /// Prefetches, in order.
    later: VecDeque<Key>,
    /// The [`Worker`] is gone: stop.
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queues: Mutex<Queues>,
    wake: Condvar,
}

impl Shared {
    /// The next job, waiting for one; `None` once closed.
    fn next(&self) -> Option<Job> {
        let mut q = self.queues.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if q.closed {
                return None;
            }
            if let Some(j) = q.now.pop_front() {
                return Some(j);
            }
            if let Some(k) = q.later.pop_front() {
                return Some(Job::Track(k));
            }
            q = self.wake.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn with(&self, f: impl FnOnce(&mut Queues)) {
        f(&mut self.queues.lock().unwrap_or_else(|e| e.into_inner()));
        self.wake.notify_one();
    }
}

/// Worker threads taking requests first, then prefetches, each in order.
pub struct Worker {
    shared: Arc<Shared>,
    rx: Receiver<Done>,
    /// Every key asked for, to send each once.
    asked: BTreeSet<Key>,
    /// The keys asked for by [`Build::request`] (or promoted to it).
    requested: BTreeSet<Key>,
    built: Vec<(Key, Arc<Track>)>,
    proved: Vec<(u64, Proved)>,
}

impl Worker {
    /// Spawn [`worker_count`] workers building with [`Generate`]. They exit
    /// when the `Worker` is dropped (after the job in hand).
    pub fn spawn() -> Worker {
        Worker::with(worker_count(), generate())
    }

    /// Spawn `n` workers (at least one) getting tracks from `source`.
    pub fn with(n: usize, source: Source) -> Worker {
        let shared = Arc::new(Shared::default());
        let (done, rx) = channel::<Done>();
        for i in 0..n.max(1) {
            let (shared, done, source) = (shared.clone(), done.clone(), source.clone());
            std::thread::Builder::new()
                .name(format!("racer-tracks-{i}"))
                .spawn(move || work(&shared, &done, &*source))
                .expect("spawn a track builder");
        }
        Worker {
            shared,
            rx,
            asked: BTreeSet::new(),
            requested: BTreeSet::new(),
            built: Vec::new(),
            proved: Vec::new(),
        }
    }

    /// Sort what has finished into tracks and proofs.
    fn drain(&mut self) {
        for d in self.rx.try_iter() {
            match d {
                Done::Track(k, t) => self.built.push((k, t)),
                Done::Proof(id, p) => self.proved.push((id, *p)),
            }
        }
    }
}

/// One worker's loop.
fn work(shared: &Shared, done: &Sender<Done>, source: &(dyn Fn(Key) -> Arc<Track> + Send + Sync)) {
    while let Some(job) = shared.next() {
        let out = match job {
            Job::Track(key) => Done::Track(key, source(key)),
            Job::Prove(id, layout) => Done::Proof(id, Box::new(layout.prove())),
        };
        if done.send(out).is_err() {
            break;
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.with(|q| q.closed = true);
        self.shared.wake.notify_all();
    }
}

impl Build for Worker {
    fn request(&mut self, key: Key) {
        if !self.requested.insert(key) {
            return;
        }
        let fresh = self.asked.insert(key);
        self.shared.with(|q| {
            // A prefetch not yet started jumps the queue; one in hand
            // finishes where it is.
            if fresh || q.later.contains(&key) {
                q.later.retain(|k| *k != key);
                q.now.push_back(Job::Track(key));
            }
        });
    }

    fn prefetch(&mut self, key: Key) {
        if self.asked.insert(key) {
            self.shared.with(|q| q.later.push_back(key));
        }
    }

    fn poll(&mut self) -> Vec<(Key, Arc<Track>)> {
        self.drain();
        std::mem::take(&mut self.built)
    }

    fn prove(&mut self, job: u64, layout: Layout) {
        self.shared
            .with(|q| q.now.push_back(Job::Prove(job, layout)));
    }

    fn proofs(&mut self) -> Vec<(u64, Proved)> {
        self.drain();
        std::mem::take(&mut self.proved)
    }
}

/// A prover for [`Inline`]: a layout in, a proof's outcome out.
pub type ProveFn = Box<dyn FnMut(&Layout) -> Proved>;

/// Builds with a function, on the calling thread, at the next poll: for
/// tests, which hand it fixture tracks. Proofs are done at the next
/// [`Build::proofs`], by [`Layout::prove`] unless [`Inline::proving_with`]
/// says otherwise.
pub struct Inline<F: FnMut(Key) -> Track> {
    make: F,
    queue: Vec<Key>,
    to_prove: Vec<(u64, Layout)>,
    prover: Option<ProveFn>,
    /// While set, proofs are held back: a slow worker. Shared, so a test can
    /// let them go after handing the builder to a game.
    pub hold: Rc<Cell<bool>>,
}

impl<F: FnMut(Key) -> Track> Inline<F> {
    /// Build with `make`.
    pub fn new(make: F) -> Self {
        Inline {
            make,
            queue: Vec::new(),
            to_prove: Vec::new(),
            prover: None,
            hold: Rc::new(Cell::new(false)),
        }
    }

    /// Prove with `f` instead of [`Layout::prove`].
    pub fn proving_with(mut self, f: impl FnMut(&Layout) -> Proved + 'static) -> Self {
        self.prover = Some(Box::new(f));
        self
    }
}

impl<F: FnMut(Key) -> Track> Build for Inline<F> {
    fn request(&mut self, key: Key) {
        if !self.queue.contains(&key) {
            self.queue.push(key);
        }
    }

    fn poll(&mut self) -> Vec<(Key, Arc<Track>)> {
        std::mem::take(&mut self.queue)
            .into_iter()
            .map(|k| (k, Arc::new((self.make)(k))))
            .collect()
    }

    fn prove(&mut self, job: u64, layout: Layout) {
        self.to_prove.push((job, layout));
    }

    fn proofs(&mut self) -> Vec<(u64, Proved)> {
        if self.hold.get() {
            return Vec::new();
        }
        std::mem::take(&mut self.to_prove)
            .into_iter()
            .map(|(job, l)| {
                let out = match &mut self.prover {
                    Some(f) => f(&l),
                    None => l.prove(),
                };
                (job, out)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_builds_what_generate_builds() {
        let mut w = Worker::spawn();
        w.request((7, 0, 0));
        let mut got = Vec::new();
        for _ in 0..600 {
            got.extend(w.poll());
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(got.len(), 1, "built within 6 s");
        let direct = Generate.track(7, 0, 0);
        assert_eq!(
            racer_sim::track::track_hash(&got[0].1),
            racer_sim::track::track_hash(&direct)
        );
    }

    #[test]
    fn the_worker_proves_in_turn_with_the_tracks_it_builds() {
        let course = racer_sim::layout::bundled::layouts().remove(0);
        let mut w = Worker::spawn();
        w.request((3, 0, 0));
        w.prove(7, course.unproven());
        let (mut tracks, mut proofs) = (Vec::new(), Vec::new());
        for _ in 0..1000 {
            tracks.extend(w.poll());
            proofs.extend(w.proofs());
            if !tracks.is_empty() && !proofs.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(tracks.len(), 1, "the track was not lost to proofs()");
        assert_eq!(proofs.len(), 1);
        let (job, out) = proofs.remove(0);
        assert_eq!(job, 7);
        assert_eq!(out.unwrap().0, course, "the same proof the course ships");
    }

    /// Poll `w` until `n` tracks are back (60 s at most).
    fn collect(w: &mut Worker, n: usize) -> Vec<(Key, Arc<Track>)> {
        let mut got = Vec::new();
        for _ in 0..6000 {
            got.extend(w.poll());
            if got.len() >= n {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        got
    }

    #[test]
    fn two_workers_build_exactly_what_one_builds() {
        let keys: Vec<Key> = vec![(11, 1, 0), (11, 1, 1), (12, 0, 0), (12, 2, 1)];
        let digests = |n: usize| {
            let mut w = Worker::with(n, generate());
            for &k in &keys {
                w.request(k);
            }
            let mut got: Vec<(Key, u64)> = collect(&mut w, keys.len())
                .iter()
                .map(|(k, t)| (*k, racer_sim::track::track_hash(t)))
                .collect();
            got.sort();
            got
        };
        let one = digests(1);
        assert_eq!(one.len(), keys.len());
        assert_eq!(one, digests(2));
    }

    #[test]
    fn requests_go_before_prefetches_and_each_key_is_built_once() {
        use std::sync::Mutex;
        let order = Arc::new(Mutex::new(Vec::new()));
        let seen = order.clone();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let open = gate.clone();
        let source: Source = Arc::new(move |k: Key| {
            // Hold the first job until everything is queued.
            let (m, cv) = &*open;
            let mut go = m.lock().unwrap();
            while !*go {
                go = cv.wait(go).unwrap();
            }
            seen.lock().unwrap().push(k);
            Arc::new(racer_sim::track::fixtures::drag_strip())
        });
        let mut w = Worker::with(1, source);
        w.prefetch((1, 0, 0));
        std::thread::sleep(std::time::Duration::from_millis(50));
        for b in 0..2 {
            w.prefetch((1, 1, b));
        }
        w.prefetch((1, 2, 0));
        w.request((9, 0, 0));
        w.request((1, 2, 0)); // promoted
        w.request((9, 0, 0)); // again: nothing
        w.prefetch((9, 0, 0)); // already asked: nothing
        {
            let (m, cv) = &*gate;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        assert_eq!(collect(&mut w, 5).len(), 5);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(w.poll().is_empty(), "each key once");
        assert_eq!(
            *order.lock().unwrap(),
            vec![(1, 0, 0), (9, 0, 0), (1, 2, 0), (1, 1, 0), (1, 1, 1)],
            "the one in hand, then the requests, then the prefetches in order"
        );
    }

    /// How long a fork's two sides take with one worker and with two:
    /// `cargo test --release -p racer-godot fork_build_time -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn fork_build_time() {
        for n in [1, 2] {
            let mut total = std::time::Duration::ZERO;
            for seed in 1..=20u64 {
                let mut w = Worker::with(n, generate());
                let t0 = std::time::Instant::now();
                w.request((seed, 1, 0));
                w.request((seed, 1, 1));
                assert_eq!(collect(&mut w, 2).len(), 2);
                total += t0.elapsed();
            }
            println!("{n} worker(s): {:?} per fork (20 forks)", total / 20);
        }
    }

    #[test]
    fn inline_builds_on_poll_once_per_key() {
        let mut n = 0;
        let mut b = Inline::new(|_| {
            n += 1;
            racer_sim::track::fixtures::drag_strip()
        });
        b.request((1, 0, 0));
        b.request((1, 0, 0));
        assert_eq!(b.poll().len(), 1);
        assert!(b.poll().is_empty());
        drop(b);
        assert_eq!(n, 1);
    }
}
