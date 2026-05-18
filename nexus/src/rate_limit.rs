// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use dropshot::ClientErrorStatusCode;
use dropshot::HttpError;
use std::collections::HashMap;
use std::sync::Mutex;

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

pub(crate) type RateLimitResult = Result<(), RateLimitKey>;

impl RateLimiter {
    pub fn new() -> Self {
        Self { states: Mutex::new(HashMap::new()) }
    }

    // Checks limits for all passed keys:
    // - if the check fails for any key, return false without incrementing anything
    // - otherwise, increment counters for all passed keys and return true
    pub fn check_and_increment(
        &self,
        keys: &[RateLimitKey],
    ) -> RateLimitResult {
        let mut states = self.states.lock().unwrap();

        for key in keys {
            if let Some(state) = states.get(key) {
                if state.count >= state.limit {
                    return Err(key.clone());
                }
            }
        }

        for key in keys {
            if let Some(state) = states.get_mut(key) {
                state.count += 1;
            } else {
                states
                    .insert(key.clone(), RateLimitState { limit: 2, count: 1 });
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn count_for_key(&self, key: &RateLimitKey) -> Option<usize> {
        self.states.lock().unwrap().get(key).map(|state| state.count)
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

        assert_not_limited(limiter.check_and_increment(&[key_a.clone()]));
        assert_not_limited(limiter.check_and_increment(&[key_a.clone()]));
        assert_limited(
            limiter.check_and_increment(&[key_a.clone()]),
            key_a.clone(),
        );

        assert_not_limited(limiter.check_and_increment(&[key_b.clone()]));
        assert_not_limited(limiter.check_and_increment(&[key_b.clone()]));
        assert_limited(
            limiter.check_and_increment(&[key_b.clone()]),
            key_b.clone(),
        );
    }

    #[test]
    fn rate_limiter_multi_key_check_is_all_or_nothing() {
        let limiter = RateLimiter::new();

        let shared = RateLimitKey::new("shared");
        let key_a = RateLimitKey::new("key_a");
        let key_b = RateLimitKey::new("key_b");

        // these two check_and_increment calls succeed, but they increment the
        // shared key's counter to the limit
        assert_not_limited(
            limiter.check_and_increment(&[key_a.clone(), shared.clone()]),
        );
        assert_not_limited(
            limiter.check_and_increment(&[key_a.clone(), shared.clone()]),
        );

        // this check_and_increment fails since it also uses the shared key
        // and should not increment any of the two counters
        assert_limited(
            limiter.check_and_increment(&[key_b.clone(), shared.clone()]),
            shared.clone(),
        );

        // because key_b's counter wasn't incremented, two check_and_increment
        // calls should still succeed and the third one should fail
        assert_not_limited(limiter.check_and_increment(&[key_b.clone()]));
        assert_not_limited(limiter.check_and_increment(&[key_b.clone()]));
        assert_limited(
            limiter.check_and_increment(&[key_b.clone()]),
            key_b.clone(),
        );
    }

    #[test]
    fn rate_limiter_counter_not_created_on_failed_multi_key_check() {
        let limiter = RateLimiter::new();

        let limited = RateLimitKey::new("limited");
        let missing = RateLimitKey::new("missing");

        // make two checks to reach the limit for "limited" key
        assert_not_limited(limiter.check_and_increment(&[limited.clone()]));
        assert_not_limited(limiter.check_and_increment(&[limited.clone()]));

        // verify that counter exists for "limited" and doesn't for "missing"
        assert_eq!(limiter.count_for_key(&limited), Some(2));
        assert_eq!(limiter.count_for_key(&missing), None);

        // make a check for both keys. This will fail since the limit was
        // reached for one key. The counter should not be created for the
        // other key.
        assert_limited(
            limiter.check_and_increment(&[missing.clone(), limited.clone()]),
            limited.clone(),
        );

        // verify that the counter stil exists for "limited" and still doesn't
        // exist for "missing"
        assert_eq!(limiter.count_for_key(&limited), Some(2));
        assert_eq!(limiter.count_for_key(&missing), None);
    }

    fn assert_not_limited(rate_limit_result: RateLimitResult) {
        assert!(rate_limit_result.is_ok());
    }

    fn assert_limited(
        rate_limit_result: RateLimitResult,
        expected_key: RateLimitKey,
    ) {
        let key = rate_limit_result
            .expect_err("expected request to be limited by key");
        assert_eq!(key, expected_key);
    }
}
