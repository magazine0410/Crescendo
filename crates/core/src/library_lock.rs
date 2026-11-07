//! Who may change library paths right now. A scan, the Forte tag editor and the
//! Forte inbox refresh must not run together: a scan racing a path-preserving
//! rename would mark the renamed file missing and add it as a new track.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryWriter {
    Scan,
    TagEditor,
    /// The background check for Forte saves, which holds the lock briefly and
    /// so never turns a user's request away by itself.
    FortePoll,
}

impl LibraryWriter {
    /// Why someone else cannot start while this writer holds the lock.
    pub fn busy_message(self) -> &'static str {
        match self {
            LibraryWriter::Scan => "A library scan is already running",
            LibraryWriter::TagEditor => "Finish the current tag edit first",
            LibraryWriter::FortePoll => "Crescendo is applying saved tag edits; try again shortly",
        }
    }
}

#[derive(Default)]
pub struct LibraryLock {
    holder: Mutex<Option<LibraryWriter>>,
    released: Condvar,
}

/// Releases the lock on every exit path, including unwinding from a panic.
pub struct LibraryGuard(Arc<LibraryLock>);

impl Drop for LibraryGuard {
    fn drop(&mut self) {
        *self
            .0
            .holder
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
        self.0.released.notify_all();
    }
}

impl LibraryLock {
    pub fn holder(&self) -> Option<LibraryWriter> {
        *self
            .holder
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Take the lock if it is free; otherwise report who holds it.
    pub fn try_acquire(
        self: &Arc<Self>,
        writer: LibraryWriter,
    ) -> Result<LibraryGuard, LibraryWriter> {
        self.acquire_waiting(writer, Duration::ZERO)
    }

    /// As [`try_acquire`](Self::try_acquire), but outlast the Forte poll for up
    /// to `patience`. A real scan or edit is refused at once.
    pub fn acquire_waiting(
        self: &Arc<Self>,
        writer: LibraryWriter,
        patience: Duration,
    ) -> Result<LibraryGuard, LibraryWriter> {
        let deadline = Instant::now() + patience;
        let mut holder = self
            .holder
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        loop {
            match *holder {
                None => {
                    *holder = Some(writer);
                    return Ok(LibraryGuard(self.clone()));
                }
                Some(LibraryWriter::FortePoll) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(LibraryWriter::FortePoll);
                    }
                    holder = self
                        .released
                        .wait_timeout(holder, left)
                        .unwrap_or_else(|poison| poison.into_inner())
                        .0;
                }
                Some(other) => return Err(other),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_names_its_holder_and_a_guard_releases_it() {
        let lock = Arc::new(LibraryLock::default());
        assert_eq!(lock.holder(), None);
        {
            let _scan = lock.try_acquire(LibraryWriter::Scan).unwrap();
            assert_eq!(lock.holder(), Some(LibraryWriter::Scan));
            assert_eq!(
                lock.try_acquire(LibraryWriter::TagEditor).err(),
                Some(LibraryWriter::Scan)
            );
        }
        assert_eq!(lock.holder(), None);
        assert!(lock.try_acquire(LibraryWriter::TagEditor).is_ok());
    }

    #[test]
    fn a_guard_releases_the_lock_when_its_holder_panics() {
        let lock = Arc::new(LibraryLock::default());
        let guard = lock.try_acquire(LibraryWriter::Scan).unwrap();
        let _ = std::thread::spawn(move || {
            let _guard = guard;
            panic!("scanner failed");
        })
        .join();
        assert_eq!(lock.holder(), None);
    }

    #[test]
    fn a_request_outlasts_the_poll_but_not_a_real_writer() {
        let lock = Arc::new(LibraryLock::default());
        let poll = lock.try_acquire(LibraryWriter::FortePoll).unwrap();
        let waiter = {
            let lock = lock.clone();
            std::thread::spawn(move || {
                lock.acquire_waiting(LibraryWriter::Scan, Duration::from_secs(10))
                    .map(|_| ())
            })
        };
        std::thread::sleep(Duration::from_millis(20));
        drop(poll);
        assert_eq!(waiter.join().unwrap(), Ok(()));

        let editor = lock.try_acquire(LibraryWriter::TagEditor).unwrap();
        let started = Instant::now();
        assert_eq!(
            lock.acquire_waiting(LibraryWriter::Scan, Duration::from_secs(10))
                .err(),
            Some(LibraryWriter::TagEditor)
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "no point waiting"
        );
        drop(editor);

        let _poll = lock.try_acquire(LibraryWriter::FortePoll).unwrap();
        assert_eq!(
            lock.acquire_waiting(LibraryWriter::Scan, Duration::from_millis(20))
                .err(),
            Some(LibraryWriter::FortePoll),
            "patience is bounded"
        );
    }
}
