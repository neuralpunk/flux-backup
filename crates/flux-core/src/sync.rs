//! Locks that don't give up on poisoning.
//!
//! A `Mutex` is poisoned when a thread panicked while holding it. In Flux they guard
//! counters and lists of results that are whole after every push, so the data is still
//! good: carry on with it rather than panic again (the panic itself is reported where it
//! happened, by the thread pool).

use std::sync::{Mutex, MutexGuard, PoisonError};

pub trait Locked<T> {
    /// The lock, poisoned or not.
    fn locked(&self) -> MutexGuard<'_, T>;
    /// The value, poisoned or not.
    fn into_value(self) -> T;
}

impl<T> Locked<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn into_value(self) -> T {
        self.into_inner().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_lock_still_gives_its_data() {
        let m = std::sync::Arc::new(Mutex::new(vec![1]));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("a worker failed");
        })
        .join();
        assert!(m.is_poisoned());
        m.locked().push(2);
        let m = std::sync::Arc::try_unwrap(m).unwrap();
        assert_eq!(m.into_value(), vec![1, 2]);
    }
}
