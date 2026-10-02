//! "The last request wins": answers to older requests are dropped.

use std::cell::Cell;

#[derive(Debug, Default)]
pub struct LatestRequest {
    current: Cell<u64>,
}

impl LatestRequest {
    pub fn new() -> LatestRequest {
        LatestRequest::default()
    }

    pub fn begin(&self) -> u64 {
        self.current.set(self.current.get() + 1);
        self.current.get()
    }

    pub fn is_current(&self, id: u64) -> bool {
        self.current.get() == id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_request_lands() {
        let latest = LatestRequest::new();
        let first = latest.begin();
        let second = latest.begin();
        assert!(!latest.is_current(first), "a late answer to the first request is dropped");
        assert!(latest.is_current(second));
    }
}
