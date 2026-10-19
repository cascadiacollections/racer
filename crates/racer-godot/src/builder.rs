//! Building tracks and proving layouts off the main thread.
//!
//! A track is proven drivable before it is shown (DESIGN §2.4), which takes
//! 70 ms to 0.7 s on a desktop (2.7 s at worst) and more on a phone: far
//! too long for a frame. The game asks for both sides of the next fork as
//! a race starts, a worker thread builds them while the player drives, and
//! the fork screen waits only if the player is faster than the builder.
//!
//! The editor's PROVE (and the check of a pasted proof) is the same kind of
//! work, tens to hundreds of milliseconds, so it goes to the same thread
//! ([`Build::prove`]), in turn with the tracks, and its result comes back
//! through [`Build::proofs`] (DESIGN §9.11).
//!
//! Building is a pure function of `(run_seed, index, branch)`, and proving
//! of the layout, so where it happens cannot change a result. The worker
//! only calls into `racer-sim`; it never touches the engine (gdext's
//! `lazy-function-tables` forbids engine calls off the main thread).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

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
    /// Start building `key`. Asking twice is harmless.
    fn request(&mut self, key: Key);
    /// The tracks finished since the last poll.
    fn poll(&mut self) -> Vec<(Key, Arc<Track>)>;
    /// Start proving `layout` as job `job`.
    fn prove(&mut self, job: u64, layout: Layout);
    /// The proofs finished since the last call.
    fn proofs(&mut self) -> Vec<(u64, Proved)>;
}

enum Job {
    Track(Key),
    Prove(u64, Layout),
}

enum Done {
    Track(Key, Arc<Track>),
    Proof(u64, Box<Proved>),
}

/// One worker thread, doing requests in order.
pub struct Worker {
    tx: Sender<Job>,
    rx: Receiver<Done>,
    built: Vec<(Key, Arc<Track>)>,
    proved: Vec<(u64, Proved)>,
}

impl Worker {
    /// Spawn the worker. It exits when the `Worker` is dropped.
    pub fn spawn() -> Worker {
        let (tx, jobs) = channel::<Job>();
        let (done, rx) = channel();
        std::thread::Builder::new()
            .name("racer-tracks".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let out = match job {
                        Job::Track(key) => Done::Track(key, Generate.track(key.0, key.1, key.2)),
                        Job::Prove(id, layout) => Done::Proof(id, Box::new(layout.prove())),
                    };
                    if done.send(out).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the track builder");
        Worker {
            tx,
            rx,
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

impl Build for Worker {
    fn request(&mut self, key: Key) {
        let _ = self.tx.send(Job::Track(key));
    }

    fn poll(&mut self) -> Vec<(Key, Arc<Track>)> {
        self.drain();
        std::mem::take(&mut self.built)
    }

    fn prove(&mut self, job: u64, layout: Layout) {
        let _ = self.tx.send(Job::Prove(job, layout));
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
