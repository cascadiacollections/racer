//! Building tracks off the main thread.
//!
//! A track is proven drivable before it is shown (DESIGN §2.4), which takes
//! 70 ms to 0.7 s on a desktop (2.7 s at worst) and more on a phone: far
//! too long for a frame. The game asks for both sides of the next fork as
//! a race starts, a worker thread builds them while the player drives, and
//! the fork screen waits only if the player is faster than the builder.
//!
//! Building is a pure function of `(run_seed, index, branch)`, so where it
//! happens cannot change a track. The worker only calls into `racer-sim`;
//! it never touches the engine (gdext's `lazy-function-tables` forbids
//! engine calls off the main thread).

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use racer_sim::run::{Generate, Tracks};
use racer_sim::track::Track;

/// Which track: `(run_seed, index, branch)`.
pub type Key = (u64, u32, u8);

/// Something that builds tracks on request and hands them back later.
pub trait Build {
    /// Start building `key`. Asking twice is harmless.
    fn request(&mut self, key: Key);
    /// The tracks finished since the last poll.
    fn poll(&mut self) -> Vec<(Key, Arc<Track>)>;
}

/// One worker thread, building requests in order.
pub struct Worker {
    tx: Sender<Key>,
    rx: Receiver<(Key, Arc<Track>)>,
}

impl Worker {
    /// Spawn the worker. It exits when the `Worker` is dropped.
    pub fn spawn() -> Worker {
        let (tx, jobs) = channel::<Key>();
        let (done, rx) = channel();
        std::thread::Builder::new()
            .name("racer-tracks".into())
            .spawn(move || {
                while let Ok(key) = jobs.recv() {
                    let track = Generate.track(key.0, key.1, key.2);
                    if done.send((key, track)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the track builder");
        Worker { tx, rx }
    }
}

impl Build for Worker {
    fn request(&mut self, key: Key) {
        let _ = self.tx.send(key);
    }

    fn poll(&mut self) -> Vec<(Key, Arc<Track>)> {
        self.rx.try_iter().collect()
    }
}

/// Builds with a function, on the calling thread, at the next poll: for
/// tests, which hand it fixture tracks.
pub struct Inline<F: FnMut(Key) -> Track> {
    make: F,
    queue: Vec<Key>,
}

impl<F: FnMut(Key) -> Track> Inline<F> {
    /// Build with `make`.
    pub fn new(make: F) -> Self {
        Inline {
            make,
            queue: Vec::new(),
        }
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
