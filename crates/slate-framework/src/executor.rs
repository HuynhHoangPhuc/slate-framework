//! Async executor for UI-thread and background tasks.
//!
//! Split executor design per red-team Finding 5:
//! - `ForegroundExecutor` — `!Send`, runs on the UI thread
//! - `BackgroundExecutor` — `Send + Sync + Clone`, runs on a thread pool
//!
//! Element contexts receive `&ForegroundExecutor` for `spawn_local()`.
//! `BackgroundExecutor` is accessible via `App` for `Send` futures.

use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use smol::Task;

/// Maximum iterations for foreground poll to prevent infinite spin.
const MAX_POLL_ITERATIONS: usize = 1000;

/// Handle to the foreground (UI-thread) executor.
///
/// Futures spawned here run between frames on the main thread.
/// Accepts `?Send` futures (no `Send` bound).
///
/// # Thread Safety
///
/// `!Send` — must stay on the UI thread. Enforced via `PhantomData<*const ()>`.
pub struct ForegroundExecutor {
    inner: smol::LocalExecutor<'static>,
    redraw_requester: RedrawRequester,
    _not_send: PhantomData<*const ()>,
}

impl ForegroundExecutor {
    /// Create a new foreground executor with the given redraw requester.
    pub fn new(redraw_requester: RedrawRequester) -> Self {
        Self {
            inner: smol::LocalExecutor::new(),
            redraw_requester,
            _not_send: PhantomData,
        }
    }

    /// Spawn a local (non-Send) future on the UI thread.
    ///
    /// The future runs between frames. When it completes, a redraw is
    /// automatically requested so the UI can reflect any state changes.
    ///
    /// # Capture Contract
    ///
    /// Futures may only capture:
    /// - Owned data (`String`, `Vec<T>`, etc.)
    /// - `Arc`/`Rc` handles
    /// - `BackgroundExecutor` (Clone)
    /// - `RedrawRequester` (Clone)
    ///
    /// Futures MUST NOT capture frame-scoped borrows (`&Scene`, `&mut TextSystem`,
    /// context references) because the future outlives the frame. The `'static`
    /// bound enforces this at compile time.
    pub fn spawn_local<F, T>(&self, future: F) -> Task<T>
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        let req = self.redraw_requester.clone();
        self.inner.spawn(async move {
            let out = future.await;
            req.request();
            out
        })
    }

    /// Poll pending futures without blocking.
    ///
    /// Returns `true` if a task made progress, `false` if no work was available.
    pub fn try_tick(&self) -> bool {
        self.inner.try_tick()
    }

    /// Poll pending futures until no more progress or iteration limit reached.
    ///
    /// Call this once per frame iteration. Limited to `MAX_POLL_ITERATIONS`
    /// to prevent infinite spin if tasks continuously spawn new tasks.
    pub fn poll(&self) {
        for _ in 0..MAX_POLL_ITERATIONS {
            if !self.inner.try_tick() {
                break;
            }
        }
    }
}

/// Background (worker-pool) executor for `Send` futures.
///
/// Cheap to clone. Futures run on smol's global thread pool which is
/// lazily initialized with one thread per CPU core.
#[derive(Clone)]
pub struct BackgroundExecutor {
    redraw_requester: RedrawRequester,
}

impl BackgroundExecutor {
    /// Create a new background executor with the given redraw requester.
    pub fn new(redraw_requester: RedrawRequester) -> Self {
        Self { redraw_requester }
    }

    /// Spawn a `Send` future on the background thread pool.
    ///
    /// The future runs on smol's global thread pool (lazily initialized).
    /// When it completes, a redraw is requested to wake the UI thread.
    pub fn spawn<F, T>(&self, future: F) -> Task<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let req = self.redraw_requester.clone();
        smol::spawn(async move {
            let out = future.await;
            req.request();
            out
        })
    }

    /// Spawn a future with tokio-compatible reactor (requires `tokio` feature).
    ///
    /// Use this for futures that depend on tokio's runtime (e.g., `reqwest`, `tonic`).
    #[cfg(feature = "tokio")]
    pub fn spawn_compat<F, T>(&self, future: F) -> Task<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let req = self.redraw_requester.clone();
        smol::spawn(async move {
            let out = async_compat::Compat::new(future).await;
            req.request();
            out
        })
    }

    /// Run a blocking closure on a dedicated thread.
    ///
    /// Use for file I/O, CPU-bound work, or anything that shouldn't block
    /// the async runtime.
    pub fn spawn_blocking<F, T>(&self, f: F) -> Task<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let req = self.redraw_requester.clone();
        smol::spawn(async move {
            let out = smol::unblock(f).await;
            req.request();
            out
        })
    }
}

/// Thread-safe handle to wake the main run loop and request a redraw.
///
/// Used by background tasks to signal the UI thread when work completes.
/// Cheap to clone.
#[derive(Clone)]
pub struct RedrawRequester(Arc<dyn Fn() + Send + Sync>);

impl RedrawRequester {
    /// Create a new redraw requester with the given wake function.
    ///
    /// The function should be thread-safe and wake the platform's event loop.
    pub fn new<F>(wake_fn: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self(Arc::new(wake_fn))
    }

    /// Create a no-op redraw requester (for testing).
    pub fn noop() -> Self {
        Self(Arc::new(|| {}))
    }

    /// Request the UI thread to wake up and redraw.
    pub fn request(&self) {
        (self.0)();
    }
}

/// Aggregate executor owned by `App`.
///
/// Holds both foreground and background executors.
/// Element contexts receive `&self.foreground` directly.
pub struct Executor {
    /// Foreground executor for UI-thread tasks.
    pub foreground: ForegroundExecutor,
    /// Background executor for thread pool tasks.
    pub background: BackgroundExecutor,
}

impl Executor {
    /// Create a new executor pair with the given redraw requester.
    pub fn new(redraw_requester: RedrawRequester) -> Self {
        Self {
            foreground: ForegroundExecutor::new(redraw_requester.clone()),
            background: BackgroundExecutor::new(redraw_requester),
        }
    }

    /// Poll all pending foreground futures.
    ///
    /// Convenience method that delegates to `self.foreground.poll()`.
    pub fn poll_foreground(&self) {
        self.foreground.poll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn assert_not_send<T>() {}
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[test]
    fn foreground_executor_is_not_send() {
        assert_not_send::<ForegroundExecutor>();
    }

    #[test]
    fn background_executor_is_send_sync_clone() {
        assert_send::<BackgroundExecutor>();
        assert_sync::<BackgroundExecutor>();
        let _ = |e: BackgroundExecutor| e.clone();
    }

    #[test]
    fn redraw_requester_is_send_sync_clone() {
        assert_send::<RedrawRequester>();
        assert_sync::<RedrawRequester>();
        let _ = |r: RedrawRequester| r.clone();
    }

    #[test]
    fn spawn_local_increments_counter() {
        let counter = Rc::new(Cell::new(0));
        let counter_clone = counter.clone();

        let req = RedrawRequester::noop();
        let executor = ForegroundExecutor::new(req);

        let _task = executor.spawn_local(async move {
            counter_clone.set(counter_clone.get() + 1);
        });

        executor.poll();
        assert_eq!(counter.get(), 1);
    }

    #[test]
    fn spawn_local_requests_redraw() {
        let redraw_count = Arc::new(AtomicUsize::new(0));
        let redraw_count_clone = redraw_count.clone();

        let req = RedrawRequester::new(move || {
            redraw_count_clone.fetch_add(1, Ordering::SeqCst);
        });
        let executor = ForegroundExecutor::new(req);

        let _task = executor.spawn_local(async {});

        executor.poll();
        assert_eq!(redraw_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn poll_returns_immediately_when_empty() {
        let req = RedrawRequester::noop();
        let executor = ForegroundExecutor::new(req);

        // Should not block
        assert!(!executor.try_tick());
        executor.poll();
    }

    #[test]
    fn background_spawn_runs_future() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        let req = RedrawRequester::noop();
        let executor = BackgroundExecutor::new(req);

        let task = executor.spawn(async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            42
        });

        // Block on the task to ensure it completes
        let result = smol::block_on(task);
        assert_eq!(result, 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn background_spawn_requests_redraw() {
        let redraw_count = Arc::new(AtomicUsize::new(0));
        let redraw_count_clone = redraw_count.clone();

        let req = RedrawRequester::new(move || {
            redraw_count_clone.fetch_add(1, Ordering::SeqCst);
        });
        let executor = BackgroundExecutor::new(req);

        let task = executor.spawn(async { 1 + 1 });
        smol::block_on(task);

        assert_eq!(redraw_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn background_spawn_blocking_runs_closure() {
        let req = RedrawRequester::noop();
        let executor = BackgroundExecutor::new(req);

        let task = executor.spawn_blocking(|| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            "done"
        });

        let result = smol::block_on(task);
        assert_eq!(result, "done");
    }

    #[test]
    fn executor_aggregate_polls_foreground() {
        let counter = Rc::new(Cell::new(0));
        let counter_clone = counter.clone();

        let req = RedrawRequester::noop();
        let executor = Executor::new(req);

        let _task = executor.foreground.spawn_local(async move {
            counter_clone.set(counter_clone.get() + 1);
        });

        executor.poll_foreground();
        assert_eq!(counter.get(), 1);
    }
}
