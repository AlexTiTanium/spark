//! `Bus<T>` — broadcast fan-out placeholder.
//!
//! P3 fills this in (subscribe + publish over `std::sync::mpsc`) when
//! the log streaming layer needs a way for the HTTP thread's per-SSE
//! receivers to share a single producer on the main thread. P1 ships
//! only the type so the rest of the crate already has the name in
//! scope and `pub use bus::Bus;` does not have to be added later.

use std::marker::PhantomData;

/// Broadcast channel placeholder. No producers, no consumers in P1 —
/// P3 replaces the body with a `Mutex<Vec<mpsc::Sender<T>>>` plus
/// `subscribe` / `publish` methods.
///
/// # Examples
///
/// ```
/// use spark_mcp::Bus;
///
/// let _: Bus<u32> = Bus::new();
/// ```
pub struct Bus<T: Clone + Send + 'static> {
    _phantom: PhantomData<fn() -> T>,
}

impl<T: Clone + Send + 'static> Bus<T> {
    /// Creates an empty bus. Does nothing useful until P3 fills in the
    /// subscriber storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<T: Clone + Send + 'static> Default for Bus<T> {
    fn default() -> Self {
        Self::new()
    }
}
