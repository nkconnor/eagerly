//! Views asyncronously retrieved values and refreshes them in the background
//! at a pre-determined interval. The view is backed by [ArcSwap](https://docs.rs/arc-swap/0.4.7/arc_swap/)
//! which provides fast, lock-free reads.
//!
//! # Example Usage
//!
//! ```ignore
//!   let user_ids = View<Vec<u32>> =
//!     view(|| async {
//!         let user_ids = database_call().await();
//!         //
//!     })
//!       .frequency(Duration::from_secs(60 * 3))
//!       .load()
//!       .await;
//! ```
//!
//! [View](struct.Cache.html) is thread-safe and implements [Clone](std::marker::Clone), which provides a
//! replica pointing to the same underlying storage.
//!
//! ```ignore
//! std::thread::spawn(|| {
//!     let user_ids = user_ids.clone();
//!     // ^ receives same updates
//! });
//! ```
#![allow(dead_code)]
use arc_swap::{ArcSwap, Guard};
use futures_ticker::Ticker;
use smol::Task;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// Creates a view builder using the provided refresh
/// that will be called to load and replenish the view.
pub fn view<V: Sync + Send + 'static, R: Refresh<Value = V> + Sync + Send + 'static>(
    r: R,
) -> Builder<V, R> {
    Builder::new(r)
}

pub trait Refresh {
    type Value;
    type Output: Future<Output = Self::Value> + Sync + Send;

    fn refresh(&self) -> Self::Output;
}

impl<F, V, E> Refresh for F
where
    V: Sync + Send,
    E: Future<Output = V> + Sync + Send,
    F: Fn() -> E,
{
    type Value = V;
    type Output = E;

    fn refresh(&self) -> Self::Output {
        (self)()
    }
}

impl<V, R> std::default::Default for Builder<V, R>
where
    R: Refresh<Value = V> + Sync + Send,
    V: Sync + Send,
{
    fn default() -> Self {
        Builder {
            refresh: None,
            frequency: None,
            phantom: std::marker::PhantomData::default(),
        }
    }
}

#[derive(Debug)]
pub struct Builder<V, R>
where
    V: Sync + Send,
    R: Refresh<Value = V> + Sync + Send,
{
    refresh: Option<R>,
    frequency: Option<Duration>,
    phantom: std::marker::PhantomData<V>,
}

impl<V, R> Builder<V, R>
where
    V: Sync + Send + 'static,
    R: Refresh<Value = V> + Sync + Send + 'static,
{
    fn new(with: R) -> Self {
        Builder {
            refresh: Some(with),
            ..Self::default()
        }
    }

    pub fn frequency(self, frequency: Duration) -> Self {
        Builder {
            frequency: Some(frequency),
            ..self
        }
    }

    /// Performs an initial load of the view and creates
    /// the interval timer that will periodically do a refresh.
    ///
    /// **Panics** if either the load function or frequency were not specified.
    pub async fn load(self) -> View<V> {
        match (self.refresh, self.frequency) {
            (Some(load), Some(freq)) => {
                // initial view load
                let value = load.refresh().await;
                let write = Arc::new(ArcSwap::new(Arc::new(value)));
                let read = write.clone();
                // background timer that will replenish the view
                // with up-to date data
                use futures::stream::StreamExt;

                // this should never complete, but,
                // can we return it to let the user spawn
                // their own task?
                let ticker = Task::spawn(async move {
                    let mut tick = Ticker::new(freq);

                    while tick.next().await.is_some() {
                        let value = load.refresh().await;
                        write.store(Arc::new(value));
                    }
                });

                View {
                    ticker: Arc::new(ticker),
                    value: read,
                    phantom: std::marker::PhantomData::default(),
                }
            }
            _ => panic!("Refresh function wasn't specified"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct View<V>
where
    V: Sync + Send,
{
    value: Arc<ArcSwap<V>>,
    ticker: Arc<Task<()>>,
    phantom: std::marker::PhantomData<V>,
}

impl<V> View<V>
where
    V: Sync + Send,
{
    pub fn read(&self) -> Guard<'static, Arc<V>> {
        self.value.load()
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use futures::future;
    use smol::Timer;
    use std::time::{Duration, Instant};

    #[test]
    fn test_clone_underlying() {
        smol::run(async {
            let start = Instant::now();

            let a = view(|| async { Instant::now() })
                .frequency(Duration::from_millis(100))
                .load()
                .await;

            std::thread::sleep(Duration::from_millis(1000));

            let b = a.clone();

            assert_eq!(**b.read(), **a.read());
            assert!((**b.read()).elapsed().as_secs() < start.elapsed().as_secs());
        });
    }

    #[test]
    fn test_view_increases() {
        // basically just tests that the value is increasing
        // over time (which it should)

        // A = P * ( 1+ (r/n))^(rt)
        fn value(p: f64, r: f64, n: f64, t: f64) -> f64 {
            p * (1_f64 + (r / n)).powf(n * t)
        }

        let principal = 100_f64;
        let rate = 0.05;
        let n = 1_f64;

        let start = Instant::now();

        smol::run(async {
            let account = view(move || {
                let elapsed = start.elapsed().as_millis() as f64;
                let v = value(principal, rate, n, elapsed / 1000_f64);
                future::ready(v)
            })
            .frequency(Duration::from_millis(100))
            .load()
            .await;

            let mut value = 0_f64; // account.read().clone();

            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(500));
                let curr = **account.read();
                assert!(curr > value);
                value = curr;
            }
        })
    }

    #[test]
    #[ignore]
    fn test_throughput() {
        let task = async {
            let a = view(|| Timer::new(Duration::from_millis(100)))
                .frequency(Duration::from_millis(500))
                .load()
                .await;

            let start = Instant::now();
            let mut handles = vec![];

            for _ in 0..16 {
                let b = a.clone();
                let th = std::thread::spawn(move || {
                    for _ in 0..300_000_000 {
                        let _ = **b.read();
                    }
                });

                handles.push(th);
            }

            for h in handles {
                h.join().unwrap();
            }

            (16 * 300_000_000) as f64 / (start.elapsed().as_millis() as f64 / 1000_f64) as f64
        };
        let rate = smol::run(task) as f64 / 1_000_000_f64;
        println!("Reads/Sec (MM): {}, Writes/Sec: .5", rate);

        let task = async {
            let a = view(|| Timer::new(Duration::from_millis(100)))
                .frequency(Duration::from_millis(500))
                .load()
                .await;

            let start = Instant::now();
            let mut handles = vec![];

            for _ in 0..16 {
                let b = a.clone();
                let th = std::thread::spawn(move || {
                    for _ in 0..200_000_000 {
                        let _ = **b.read();
                    }
                });

                handles.push(th);
            }

            for h in handles {
                h.join().unwrap();
            }
            let elapsed = start.elapsed().as_millis();
            let reads = 16_u128 * 200_000_000_u128;

            (reads, elapsed)
        };

        let (reads, elapsed) = smol::run(task);
        let rate = (reads / 1_000_000) as f64 / (elapsed as f64 / 1000.0);

        println!(
            "Reads/Sec (MM): {}, Writes/Sec: .5, Elapsed: {}, Reads (MM): {}",
            rate, elapsed, reads
        );
    }
}
