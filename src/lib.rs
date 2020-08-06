//! Caches values in advance and asyncronously refreshes the value
//! at a pre-determined frequency. The cache is backed by [ArcSwap](https://docs.rs/arc-swap/0.4.7/arc_swap/)
//! which provides fast, lock-free reads.
//!
//! # Example Usage
//!
//! ```no_run
//!   let user_ids = Cache<Vec<u32>, _> =
//!     cache(|| async {
//!         vec![1,2,3] // expensive database call happens here
//!     })
//!       .frequency(Duration::from_secs(60 * 3))
//!       .load()
//!       .await;
//!
//!   assert_eq!(**user_ids.read(), vec![1,2,3])
//! ```
//!
#![allow(dead_code)]
use arc_swap::{ArcSwap, Guard};
use futures_ticker::Ticker;
use smol::Task;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// Creates a cache builder using the provided refresh
/// that will be called to load and replenish the cache.
pub fn cache<V: Sync + Send + 'static, R: Refresh<Value = V> + Sync + Send + 'static>(
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

    /// Performs an initial load of the cache and creates
    /// the interval timer that will periodically do a refresh.
    ///
    /// **Panics** if either the load function or frequency were not specified.
    pub async fn load(self) -> Cache<V, R> {
        match (self.refresh, self.frequency) {
            (Some(load), Some(freq)) => {
                // initial cache load
                let value = load.refresh().await;
                let write = Arc::new(ArcSwap::new(Arc::new(value)));
                let read = write.clone();
                // background timer that will replenish the cache
                // with up-to date data
                use futures::stream::StreamExt;

                let ticker = Task::spawn(async move {
                    let mut tick = Ticker::new(freq);

                    while tick.next().await.is_some() {
                        let value = load.refresh().await;
                        write.store(Arc::new(value));
                    }
                });

                Cache {
                    ticker,
                    value: read,
                    phantom: std::marker::PhantomData::default(),
                }
            }
            _ => panic!("Refresh function wasn't specified"),
        }
    }
}

#[derive(Debug)]
pub struct Cache<V, R>
where
    V: Sync + Send,
    R: Refresh<Value = V>,
{
    ticker: Task<()>,
    value: Arc<ArcSwap<V>>,
    phantom: std::marker::PhantomData<(R, V)>,
}

impl<V, R> Cache<V, R>
where
    V: Sync + Send,
    R: Refresh<Value = V>,
{
    pub fn read(&self) -> Guard<'static, Arc<V>> {
        self.value.load()
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use futures::future;

    #[test]
    fn account_should_increase_over_time() {
        // basically just tests that the value is increasing
        // over time (which it should)
        use std::time::{Duration, Instant};

        // A = P * ( 1+ (r/n))^(rt)
        fn value(p: f64, r: f64, n: f64, t: f64) -> f64 {
            p * (1_f64 + (r / n)).powf(n * t)
        }

        let principal = 100_f64;
        let rate = 0.05;
        let n = 1_f64;

        let start = Instant::now();

        smol::run(async {
            let account = cache(move || {
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
}
