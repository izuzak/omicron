// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use dropshot::ClientErrorStatusCode;
use dropshot::HttpError;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub(crate) struct RateLimitKey(String);

impl RateLimitKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

// State for a single fixed-window rate limiter:
// - the count of observed events
// - the start of the most recent window
#[derive(Debug)]
pub struct RateLimitState {
    count: usize,
    window_started_at: Instant,
}

// A check against a single fixed-window rate-limiter:
// - which key needs to be checked
// - what the "policy" says is the limit for that key
// - what the "policy" says is the window duration for that key
#[derive(Clone, Debug)]
pub struct RateLimitCheck {
    key: RateLimitKey,
    limit: usize,
    window: Duration,
}

impl RateLimitCheck {
    pub(crate) fn new(
        key: RateLimitKey,
        limit: usize,
        window: Duration,
    ) -> Self {
        Self { key, limit, window }
    }
}

// What we return in case a limit has been reached
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RateLimitExceeded {
    pub exceeded: Vec<RateLimitExceededKey>, // exceeded keys info
    pub retry_after: Duration, // maximum retry_after from all exceeded keys
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RateLimitExceededKey {
    pub key: RateLimitKey,
    pub limit: usize,
    pub retry_after: Duration,
}

// A container for all rate limit counter. Currently the whole container is
// wrapped in a mutex even though rate limit checks target only specific keys.
// This is good enough for now and simpler than juggling locks for individual
// keys.
pub(crate) struct RateLimiter {
    states: Mutex<HashMap<RateLimitKey, RateLimitState>>,
}

pub(crate) type RateLimitResult = Result<(), RateLimitExceeded>;

impl RateLimiter {
    pub fn new() -> Self {
        Self { states: Mutex::new(HashMap::new()) }
    }

    // Public method for checking limits for a list of keys. Calls the internal
    // method with the time to set to the current instant
    pub fn check_and_increment(
        &self,
        checks: &[RateLimitCheck],
    ) -> RateLimitResult {
        self.check_and_increment_at(checks, Instant::now())
    }

    // Internal method for checking limits for a list of keys:
    // - accepts a time instant for testing
    // - if the check fails for any key that has a non-expired window:
    //     - return Err without incrementing anything
    // - otherwise:
    //     - update the start of expired windows
    //     - increment counters for all passed keys
    //     - return Ok
    fn check_and_increment_at(
        &self,
        checks: &[RateLimitCheck],
        now: Instant,
    ) -> RateLimitResult {
        let mut states = self.states.lock().unwrap();
        let mut exceeded: Vec<RateLimitExceededKey> = Vec::new();

        for check in checks {
            if let Some(state) = states.get(&check.key) {
                let elapsed = now.duration_since(state.window_started_at);

                if (state.count >= check.limit)
                    && (now.duration_since(state.window_started_at)
                        < check.window)
                {
                    let retry_after = check.window - elapsed;

                    exceeded.push(RateLimitExceededKey {
                        key: check.key.clone(),
                        limit: check.limit,
                        retry_after,
                    });
                }
            }
        }

        // check the list of exceeded limits and find the one for which the
        // window will expire last -- this is the duration the consumer needs
        // to really wait for before retrying the same exact request
        if !exceeded.is_empty() {
            let retry_after =
                exceeded.iter().map(|e| e.retry_after).max().unwrap(); // we know that there are some items, so we can unwrap

            return Err(RateLimitExceeded { exceeded, retry_after });
        }

        for check in checks {
            if let Some(state) = states.get_mut(&check.key) {
                if now.duration_since(state.window_started_at) >= check.window {
                    state.window_started_at = now;
                    state.count = 1;
                } else {
                    state.count += 1;
                }
            } else {
                states.insert(
                    check.key.clone(),
                    RateLimitState { count: 1, window_started_at: now },
                );
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub fn count_for_key(&self, key: &RateLimitKey) -> Option<usize> {
        self.states.lock().unwrap().get(key).map(|state| state.count)
    }

    #[cfg(test)]
    pub fn reset(&self) {
        self.states.lock().unwrap().clear();
    }

    #[cfg(test)]
    pub fn reset_key(&self, key: &RateLimitKey) {
        self.states.lock().unwrap().remove(key);
    }
}

pub(crate) fn rate_limit_error(exceeded: RateLimitExceeded) -> HttpError {
    HttpError::for_client_error_with_status(
        Some(String::from("RateLimitExceeded")),
        ClientErrorStatusCode::TOO_MANY_REQUESTS,
    )
    .with_header(
        http::header::RETRY_AFTER,
        exceeded.retry_after.as_secs().max(1).to_string(),
    )
    .expect("Retry-After header value is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_independent_keys_do_not_affect_each_other() {
        let limiter = RateLimiter::new();

        let key_a = RateLimitKey::new("endpoint_a");
        let key_b = RateLimitKey::new("endpoint_b");

        let check_key_a = RateLimitCheck {
            key: key_a.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };
        let check_key_b = RateLimitCheck {
            key: key_b.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };

        assert_not_limited(limiter.check_and_increment(&[check_key_a.clone()]));
        assert_not_limited(limiter.check_and_increment(&[check_key_a.clone()]));
        assert_limited(
            limiter.check_and_increment(&[check_key_a.clone()]),
            &[key_a.clone()],
            None,
        );

        assert_not_limited(limiter.check_and_increment(&[check_key_b.clone()]));
        assert_not_limited(limiter.check_and_increment(&[check_key_b.clone()]));
        assert_limited(
            limiter.check_and_increment(&[check_key_b.clone()]),
            &[key_b.clone()],
            None,
        );
    }

    #[test]
    fn rate_limiter_multi_key_check_is_all_or_nothing() {
        let limiter = RateLimiter::new();

        let shared = RateLimitKey::new("shared");
        let key_a = RateLimitKey::new("key_a");
        let key_b = RateLimitKey::new("key_b");

        let check_shared = RateLimitCheck {
            key: shared.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };
        let check_key_a = RateLimitCheck {
            key: key_a.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };
        let check_key_b = RateLimitCheck {
            key: key_b.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };

        // these two check_and_increment calls succeed, but they increment the
        // shared key's counter to the limit
        assert_not_limited(
            limiter.check_and_increment(&[
                check_key_a.clone(),
                check_shared.clone(),
            ]),
        );
        assert_not_limited(
            limiter.check_and_increment(&[
                check_key_a.clone(),
                check_shared.clone(),
            ]),
        );

        // this check_and_increment fails since it also uses the shared key
        // and should not increment any of the two counters
        assert_limited(
            limiter.check_and_increment(&[
                check_key_b.clone(),
                check_shared.clone(),
            ]),
            &[shared.clone()],
            None,
        );

        // because key_b's counter wasn't incremented, two check_and_increment
        // calls should still succeed and the third one should fail
        assert_not_limited(limiter.check_and_increment(&[check_key_b.clone()]));
        assert_not_limited(limiter.check_and_increment(&[check_key_b.clone()]));
        assert_limited(
            limiter.check_and_increment(&[check_key_b.clone()]),
            &[key_b.clone()],
            None,
        );
    }

    #[test]
    fn rate_limiter_counter_not_created_on_failed_multi_key_check() {
        let limiter = RateLimiter::new();

        let limited = RateLimitKey::new("limited");
        let missing = RateLimitKey::new("missing");

        let check_limited = RateLimitCheck {
            key: limited.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };
        let check_missing = RateLimitCheck {
            key: missing.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };

        // make two checks to reach the limit for "limited" key
        assert_not_limited(
            limiter.check_and_increment(&[check_limited.clone()]),
        );
        assert_not_limited(
            limiter.check_and_increment(&[check_limited.clone()]),
        );

        // verify that counter exists for "limited" and doesn't for "missing"
        assert_eq!(limiter.count_for_key(&limited), Some(2));
        assert_eq!(limiter.count_for_key(&missing), None);

        // make a check for both keys. This will fail since the limit was
        // reached for one key. The counter should not be created for the
        // other key.
        assert_limited(
            limiter.check_and_increment(&[
                check_missing.clone(),
                check_limited.clone(),
            ]),
            &[limited.clone()],
            None,
        );

        // verify that the counter still exists for "limited" and still doesn't
        // exist for "missing"
        assert_eq!(limiter.count_for_key(&limited), Some(2));
        assert_eq!(limiter.count_for_key(&missing), None);
    }

    #[test]
    fn rate_limiter_reset_removes_keys() {
        let limiter = RateLimiter::new();

        let key_a = RateLimitKey::new("key_a");
        let key_b = RateLimitKey::new("key_b");

        let check_key_a = RateLimitCheck {
            key: key_a.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };
        let check_key_b = RateLimitCheck {
            key: key_b.clone(),
            limit: 2,
            window: Duration::from_secs(3600),
        };

        // confirm that there are no counters for the keys
        assert_eq!(limiter.count_for_key(&key_a), None);
        assert_eq!(limiter.count_for_key(&key_b), None);

        // trigger checks so that the counters are created
        assert_not_limited(limiter.check_and_increment(&[check_key_a.clone()]));
        assert_not_limited(limiter.check_and_increment(&[check_key_b.clone()]));

        // confirm that counters now exist
        assert_eq!(limiter.count_for_key(&key_a), Some(1));
        assert_eq!(limiter.count_for_key(&key_b), Some(1));

        // call reset to remove keys
        limiter.reset();

        // confirm that there are no counters for the keys
        assert_eq!(limiter.count_for_key(&key_a), None);
        assert_eq!(limiter.count_for_key(&key_b), None);

        // trigger checks so that the counters are created again
        assert_not_limited(limiter.check_and_increment(&[check_key_a.clone()]));
        assert_not_limited(limiter.check_and_increment(&[check_key_b.clone()]));

        // confirm that counters exist again
        assert_eq!(limiter.count_for_key(&key_a), Some(1));
        assert_eq!(limiter.count_for_key(&key_b), Some(1));

        // call reset_key to delete one counter
        limiter.reset_key(&key_a);

        // confirm that counter for key_a was deleted and still exist for key_b
        assert_eq!(limiter.count_for_key(&key_a), None);
        assert_eq!(limiter.count_for_key(&key_b), Some(1));

        // call reset_key to delete other counter
        limiter.reset_key(&key_b);

        // confirm that counter for key_b was deleted as well
        assert_eq!(limiter.count_for_key(&key_a), None);
        assert_eq!(limiter.count_for_key(&key_b), None);
    }

    #[test]
    fn rate_limiter_resets_counter_after_window_expires() {
        let limiter = RateLimiter::new();
        let key = RateLimitKey::new("some-key");
        let window = Duration::from_secs(60);
        let check = RateLimitCheck::new(key.clone(), 1, window);
        let now = Instant::now();

        // make a check to reach the limit
        assert_not_limited(
            limiter.check_and_increment_at(&[check.clone()], now),
        );

        // verify that the following check within the same window is limited
        assert_limited(
            limiter.check_and_increment_at(&[check.clone()], now),
            &[key.clone()],
            None,
        );

        // verify that a check is limited if made just before window expires
        assert_limited(
            limiter.check_and_increment_at(
                &[check.clone()],
                now + window - Duration::from_nanos(1),
            ),
            &[key.clone()],
            None,
        );

        // verify that the following check after the window expires is allowed
        assert_not_limited(
            limiter.check_and_increment_at(&[check.clone()], now + window),
        );
    }

    #[test]
    fn rate_limiter_returns_rate_limit_exceeded_with_max_retry_after() {
        let limiter = RateLimiter::new();
        let key_a = RateLimitKey::new("key_a");
        let key_b = RateLimitKey::new("key_b");
        let key_c = RateLimitKey::new("key_c");
        let now = Instant::now();

        let check_key_a = RateLimitCheck {
            key: key_a.clone(),
            limit: 2,
            window: Duration::from_secs(60),
        };
        let check_key_b = RateLimitCheck {
            key: key_b.clone(),
            limit: 5,
            window: Duration::from_secs(100),
        };
        let check_key_c = RateLimitCheck {
            key: key_c.clone(),
            limit: 2,
            window: Duration::from_secs(120),
        };

        let checks =
            &[check_key_a.clone(), check_key_b.clone(), check_key_c.clone()];

        // make two requests to hit the limits for key_a and key_c
        assert_not_limited(limiter.check_and_increment_at(checks, now));
        assert_not_limited(
            limiter
                .check_and_increment_at(checks, now + Duration::from_secs(1)),
        );

        // verify that limits are hit and which ones and that the max time is returned
        assert_limited(
            limiter
                .check_and_increment_at(checks, now + Duration::from_secs(2)),
            &[key_a.clone(), key_c.clone()],
            Some(Duration::from_secs(118)),
        );
    }

    fn assert_not_limited(rate_limit_result: RateLimitResult) {
        assert!(rate_limit_result.is_ok());
    }

    fn assert_limited(
        rate_limit_result: RateLimitResult,
        expected_keys: &[RateLimitKey],
        expected_retry_after: Option<Duration>,
    ) {
        let exceeded_info =
            rate_limit_result.expect_err("expected request to be limited");

        let exceeded_keys = exceeded_info
            .exceeded
            .iter()
            .map(|exceeded_key_info| exceeded_key_info.key.clone())
            .collect::<Vec<_>>();

        assert_same_items(&exceeded_keys, expected_keys);
        if let Some(expected_retry_after) = expected_retry_after {
            assert_eq!(exceeded_info.retry_after, expected_retry_after);
        }
    }

    fn assert_same_items<T>(actual: &[T], expected: &[T])
    where
        T: Clone + Ord + std::fmt::Debug,
    {
        let mut actual = actual.to_vec();
        let mut expected = expected.to_vec();

        actual.sort();
        expected.sort();

        assert_eq!(actual, expected);
    }
}
