use std::collections::HashMap;
use std::sync::Mutex;
use dropshot::HttpError;
use dropshot::ClientErrorStatusCode;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct RateLimitKey(String);

impl RateLimitKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

#[derive(Debug)]
pub struct RateLimitState {
    limit: usize,
    count: usize,
}

pub(crate) struct RateLimiter {
    states: Mutex<HashMap<RateLimitKey, RateLimitState>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self { states: Mutex::new(HashMap::new()) }
    }

    // Checks limits for all passed keys:
    // - if the check fails for any key, return false without incrementing anything
    // - otherwise, increment counters for all passed keys and return true
    pub fn check_and_increment(&self, keys: &[RateLimitKey]) -> bool {
        let mut states = self.states.lock().unwrap();

        for key in keys {
            if let Some(state) = states.get(key) {
                if state.count >= state.limit {
                    return false;
                }
            } else {
                states
                    .insert(key.clone(), RateLimitState { limit: 2, count: 0 });
            }
        }

        for key in keys {
            states.get_mut(&key).unwrap().count += 1;
        }

        true
    }
}

pub(crate) fn rate_limit_error() -> HttpError {
    HttpError::for_client_error_with_status(
        Some(String::from("RateLimitExceeded")),
        ClientErrorStatusCode::TOO_MANY_REQUESTS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_independent_keys_do_not_affect_each_other() {
        let limiter = RateLimiter::new();

        let key_a = RateLimitKey::new("endpoint_a");
        let key_b = RateLimitKey::new("endpoint_b");

        assert!(limiter.check_and_increment(&[key_a.clone()]));
        assert!(limiter.check_and_increment(&[key_a.clone()]));
        assert!(!limiter.check_and_increment(&[key_a]));

        assert!(limiter.check_and_increment(&[key_b.clone()]));
        assert!(limiter.check_and_increment(&[key_b.clone()]));
        assert!(!limiter.check_and_increment(&[key_b]));
    }
}
