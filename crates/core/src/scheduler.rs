use crate::traits::RateLimiter;
use futures::future::BoxFuture;
use governor::{
    clock::DefaultClock,
    middleware::NoOpMiddleware,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovRateLimiter,
};
use std::{
    collections::HashMap,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};
use tracing::debug;

type GovernorLimiter = GovRateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

/// Per-domain rate limiter backed by the `governor` crate.
pub struct PerDomainRateLimiter {
    /// Requests per second applied to every domain.
    rps: NonZeroU32,
    limiters: Mutex<HashMap<String, Arc<GovernorLimiter>>>,
}

impl PerDomainRateLimiter {
    /// Creates a new rate limiter with the given requests-per-second ceiling.
    /// `rps` is clamped to at least 1.
    pub fn new(rps: f64) -> Self {
        let rps_u32 = (rps.ceil() as u32).max(1);
        Self {
            rps: NonZeroU32::new(rps_u32)
                .expect("rps is always >= 1 after the max(1) guard above"),
            limiters: Mutex::new(HashMap::new()),
        }
    }

    fn get_or_create(&self, domain: &str) -> Arc<GovernorLimiter> {
        let mut map = self.limiters.lock().expect("scheduler mutex is never poisoned");
        if let Some(limiter) = map.get(domain) {
            return Arc::clone(limiter);
        }
        let quota = Quota::per_second(self.rps);
        let limiter = Arc::new(GovRateLimiter::direct(quota));
        map.insert(domain.to_owned(), Arc::clone(&limiter));
        limiter
    }

    async fn acquire_inner(&self, domain: &str) {
        let limiter = self.get_or_create(domain);
        loop {
            match limiter.check() {
                Ok(()) => break,
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }
}

impl RateLimiter for PerDomainRateLimiter {
    fn acquire<'a>(&'a self, domain: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(self.acquire_inner(domain))
    }

    fn report_rate_limit(&self, domain: &str) {
        debug!(domain, "rate-limit response received — backing off");
    }
}
