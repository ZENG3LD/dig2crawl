use std::sync::atomic::{AtomicU64, Ordering};

/// Tracks crawl budget constraints (max pages and max depth).
pub struct CrawlBudget {
    max_pages: Option<u64>,
    max_depth: Option<usize>,
    pages_fetched: AtomicU64,
}

impl CrawlBudget {
    pub fn new(max_pages: Option<usize>, max_depth: Option<usize>) -> Self {
        Self {
            max_pages: max_pages.map(|n| n as u64),
            max_depth,
            pages_fetched: AtomicU64::new(0),
        }
    }

    /// Returns true if the engine may fetch a URL at this depth.
    pub fn can_fetch(&self, depth: usize) -> bool {
        if let Some(max_depth) = self.max_depth {
            if depth > max_depth {
                return false;
            }
        }
        if let Some(max_pages) = self.max_pages {
            if self.pages_fetched.load(Ordering::Relaxed) >= max_pages {
                return false;
            }
        }
        true
    }

    /// Increment the pages-fetched counter.
    pub fn record_fetch(&self) {
        self.pages_fetched.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns true when the page budget is exhausted.
    pub fn is_exhausted(&self) -> bool {
        if let Some(max_pages) = self.max_pages {
            return self.pages_fetched.load(Ordering::Relaxed) >= max_pages;
        }
        false
    }

    pub fn pages_fetched(&self) -> u64 {
        self.pages_fetched.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_limit() {
        let budget = CrawlBudget::new(Some(3), None);
        assert!(budget.can_fetch(0));
        budget.record_fetch();
        budget.record_fetch();
        budget.record_fetch();
        assert!(!budget.can_fetch(0));
        assert!(budget.is_exhausted());
    }

    #[test]
    fn test_depth_limit() {
        let budget = CrawlBudget::new(None, Some(2));
        assert!(budget.can_fetch(0));
        assert!(budget.can_fetch(2));
        assert!(!budget.can_fetch(3));
    }

    #[test]
    fn test_no_limits() {
        let budget = CrawlBudget::new(None, None);
        for _ in 0..1000 {
            budget.record_fetch();
        }
        assert!(!budget.is_exhausted());
        assert!(budget.can_fetch(9999));
    }
}
