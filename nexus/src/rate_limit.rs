// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use anyhow::Context;
use dropshot::ClientErrorStatusCode;
use dropshot::HttpError;
use nexus_db_model as db_model;
use nexus_types::external_api::rate_limit as external_rate_limit;
use nexus_types::identity::Resource;
use std::borrow::Borrow;
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

    pub(crate) fn key(&self) -> &RateLimitKey {
        &self.key
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
        checks: &[impl Borrow<RateLimitCheck>],
    ) -> RateLimitResult {
        self.check_and_increment_at(checks, Instant::now())
    }

    // Internal method for checking limits for a list of keys:
    // - accepts a time instant for testing
    // - if the check fails for any key that has a non-expired window:
    //     - return Err without incrementing any counters
    //     - include information on all check that failed
    // - otherwise:
    //     - update the start of expired windows
    //     - increment counters for all passed keys
    //     - return Ok
    fn check_and_increment_at(
        &self,
        checks: &[impl Borrow<RateLimitCheck>],
        now: Instant,
    ) -> RateLimitResult {
        let mut states = self.states.lock().unwrap();
        let mut exceeded: Vec<RateLimitExceededKey> = Vec::new();

        for check in checks {
            let check = check.borrow();
            if let Some(state) = states.get(&check.key) {
                let elapsed = now.duration_since(state.window_started_at);

                // a limit is hit if both of these are true:
                // - the count for the current window is over the limit
                // - the current window has not expired i.e. now is within the
                //   window duration since the start of the window
                // otherwise, we either need to start a new window or can
                // increment the current window's coounter
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

        // if no limits were hit, we need to:
        // - start new windows for keys that previously had expired windows
        // - increment counters for keys that had a non-expired window
        for check in checks {
            let check = check.borrow();

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

// Matchers are ways a policy expresses which requests it applies to. These
// matchers look at specific parts of the request, e.g. the endpoint, the
// method, the authenticated user, etc.
//
// For a start, I'm using matchers based on the endpoint, the method, and a
// global matcher which matches all requests (e.g. for a global rate limit).
// This list could be expanded to cover more things, like identity, resource
// groups, silos, etc.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MatchPredicate {
    Endpoint {
        any_of: Vec<String>,
    },
    #[allow(dead_code)]
    HttpMethod {
        any_of: Vec<http::Method>,
    },
    Global,
}

// Different types of pieces from which a rate limit key can be constructed
// from a request. Again, this is just a starting point which defines a
// string literal piece (not based on the request), the endpoint id, and the
// http method. More pieces can be added in the future, e.g. identity, resource
// group, etc.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RateLimitKeyPart {
    Literal(String),
    Endpoint,
    #[allow(dead_code)]
    HttpMethod,
}

// How a rate limit policy defines the quota for a specific rate limit key.
// In other words, this defines the X requests per Y time window limit.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RateLimitQuota {
    limit: usize,
    window: Duration,
}

impl RateLimitQuota {
    pub(crate) fn new(limit: usize, window: Duration) -> Self {
        Self { limit, window }
    }
}

// A rate limit policy defines:
// - a policy id (a human-friendly, unique name)
// - which requests it applies to (via matchers)
// - how to construct the rate limit key for matching requests (via key_parts)
// - what the quota is for the constructed key (via the quota)
//
// The check_for method uses these pieces to determine if the policy applies to
// a given request context and, if so, returns the corresponding RateLimitCheck
// that can be used to check against the RateLimiter.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RateLimitPolicy {
    id: String,
    matchers: Vec<MatchPredicate>,
    quota: RateLimitQuota,
    key_parts: Vec<RateLimitKeyPart>,
}

impl RateLimitPolicy {
    pub(crate) fn new(
        id: impl Into<String>,
        matchers: Vec<MatchPredicate>,
        quota: RateLimitQuota,
        key_parts: Vec<RateLimitKeyPart>,
    ) -> Self {
        Self { id: id.into(), matchers, quota, key_parts }
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    // create the checkfor this policy and a request based on its context
    // this involves:
    // - checking that all policy matchers match the information in the context
    // - constructing a key from the policy's key template i.e. key parts
    // - returning a check with that key and limit + window duration
    pub(crate) fn check_for(
        &self,
        ctx: &RateLimitRequestContext,
    ) -> Option<RateLimitCheck> {
        for matcher in &self.matchers {
            match matcher {
                MatchPredicate::Endpoint { any_of } => {
                    if !any_of.iter().any(|endpoint| endpoint == &ctx.endpoint)
                    {
                        return None;
                    }
                }
                MatchPredicate::HttpMethod { any_of } => {
                    if !any_of.iter().any(|method| method == &ctx.method) {
                        return None;
                    }
                }
                MatchPredicate::Global => {}
            }
        }

        let key = self
            .key_parts
            .iter()
            .map(|part| match part {
                RateLimitKeyPart::Literal(value) => value.clone(),
                RateLimitKeyPart::Endpoint => ctx.endpoint.clone(),
                RateLimitKeyPart::HttpMethod => ctx.method.as_str().to_string(),
            })
            .collect::<Vec<_>>()
            .join(":");

        Some(RateLimitCheck::new(
            RateLimitKey::new(key),
            self.quota.limit,
            self.quota.window,
        ))
    }
}

#[derive(Debug, thiserror::Error)]
enum RateLimitPolicyCompileError {
    #[error("invalid HTTP method {method:?}: {source}")]
    InvalidHttpMethod { method: String, source: http::method::InvalidMethod },
    #[error(
        "invalid quota_limit {value} -- must be greater than 0 and fit in usize"
    )]
    InvalidQuotaLimit { value: i64 },
    #[error("invalid quota_window_seconds {value} -- must be greater than 0")]
    InvalidQuotaWindow { value: i64 },
}

impl TryFrom<&db_model::RateLimitPolicy> for RateLimitPolicy {
    type Error = anyhow::Error;

    // compile/convert a db rate limit policy model into the internal object
    // used for tracking/enforcing rate limits for API endpoints. we reuse some
    // of the api types (e.g. for matchers and key parts) for deserializing
    // json and converting to the required struct. in the future, we might do
    // different things, e.g. deserializing in the db model type or exposing
    // typed data via db model methods, to simplify this conversion.
    fn try_from(
        policy: &db_model::RateLimitPolicy,
    ) -> Result<Self, Self::Error> {
        let policy_name = policy.name().to_string();

        // convert matchers from json
        let matchers = serde_json::from_value::<
            Vec<external_rate_limit::RateLimitMatcher>,
        >(policy.matchers.clone())
        .with_context(|| {
            format!(
                "couldn't deserialize matchers for rate limit policy {policy_name}"
            )
        })?
        .into_iter()
        .map(compile_matcher)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| {
            format!("couldn't compile rate limit policy {policy_name}")
        })?;

        // convert key parts from json
        let key_parts = serde_json::from_value::<
            Vec<external_rate_limit::RateLimitKeyPart>,
        >(policy.key_parts.clone())
        .with_context(|| {
            format!(
                "couldn't deserialize key parts for rate limit policy {policy_name}"
            )
        })?
        .into_iter()
        .map(compile_key_part)
        .collect();

        // convert quota limit and window duration
        let quota_limit = compile_quota_limit(policy.quota_limit)
            .with_context(|| {
                format!("couldn't compile rate limit policy {policy_name}")
            })?;

        let quota_window = compile_quota_window(policy.quota_window_seconds)
            .with_context(|| {
                format!("couldn't compile rate limit policy {policy_name}")
            })?;

        // use the name of the policy as the id of the compiled object. the
        // name is unique so it should be fine. in the future, might switch
        // it to the uuid id from the db table or include both if both are
        // useful.
        Ok(Self::new(
            policy_name,
            matchers,
            RateLimitQuota::new(quota_limit, quota_window),
            key_parts,
        ))
    }
}

fn compile_matcher(
    matcher: external_rate_limit::RateLimitMatcher,
) -> Result<MatchPredicate, RateLimitPolicyCompileError> {
    match matcher {
        external_rate_limit::RateLimitMatcher::Endpoint { any_of } => {
            Ok(MatchPredicate::Endpoint { any_of })
        }
        external_rate_limit::RateLimitMatcher::HttpMethod { any_of } => {
            let any_of = any_of
                .into_iter()
                .map(|method| {
                    method.parse().map_err(|source| {
                        RateLimitPolicyCompileError::InvalidHttpMethod {
                            method,
                            source,
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(MatchPredicate::HttpMethod { any_of })
        }
        external_rate_limit::RateLimitMatcher::Global => {
            Ok(MatchPredicate::Global)
        }
    }
}

fn compile_key_part(
    key_part: external_rate_limit::RateLimitKeyPart,
) -> RateLimitKeyPart {
    match key_part {
        external_rate_limit::RateLimitKeyPart::Literal { value } => {
            RateLimitKeyPart::Literal(value)
        }
        external_rate_limit::RateLimitKeyPart::Endpoint => {
            RateLimitKeyPart::Endpoint
        }
        external_rate_limit::RateLimitKeyPart::HttpMethod => {
            RateLimitKeyPart::HttpMethod
        }
    }
}

fn compile_quota_limit(
    value: i64,
) -> Result<usize, RateLimitPolicyCompileError> {
    if value <= 0 {
        return Err(RateLimitPolicyCompileError::InvalidQuotaLimit { value });
    }
    usize::try_from(value)
        .map_err(|_| RateLimitPolicyCompileError::InvalidQuotaLimit { value })
}

fn compile_quota_window(
    value: i64,
) -> Result<Duration, RateLimitPolicyCompileError> {
    if value <= 0 {
        return Err(RateLimitPolicyCompileError::InvalidQuotaWindow { value });
    }
    Ok(Duration::from_secs(value as u64))
}

// The parts of the request context which are needed by for the matching and
// key construction. Again, very reduced for now, and would be expanded in the
// future to include more things like identity.
pub(crate) struct RateLimitRequestContext {
    pub endpoint: String,
    method: http::Method,
}

impl RateLimitRequestContext {
    pub(crate) fn new(
        endpoint: impl Into<String>,
        method: http::Method,
    ) -> Self {
        Self { endpoint: endpoint.into(), method }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omicron_common::api::external::IdentityMetadataCreateParams;
    use serde_json::json;
    use uuid::Uuid;

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

        assert_not_limited(limiter.check_and_increment(&[&check_key_a]));
        assert_not_limited(limiter.check_and_increment(&[&check_key_a]));
        assert_limited(
            limiter.check_and_increment(&[check_key_a.clone()]),
            &[key_a.clone()],
            None,
        );

        assert_not_limited(limiter.check_and_increment(&[&check_key_b]));
        assert_not_limited(limiter.check_and_increment(&[&check_key_b]));
        assert_limited(
            limiter.check_and_increment(&[&check_key_b]),
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
            limiter.check_and_increment(&[&check_key_a, &check_shared]),
        );
        assert_not_limited(
            limiter.check_and_increment(&[&check_key_a, &check_shared]),
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
        assert_not_limited(limiter.check_and_increment(&[&check_key_b]));
        assert_not_limited(limiter.check_and_increment(&[&check_key_b]));
        assert_limited(
            limiter.check_and_increment(&[&check_key_b]),
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
        assert_not_limited(limiter.check_and_increment(&[&check_limited]));
        assert_not_limited(limiter.check_and_increment(&[&check_limited]));

        // verify that counter exists for "limited" and doesn't for "missing"
        assert_eq!(limiter.count_for_key(&limited), Some(2));
        assert_eq!(limiter.count_for_key(&missing), None);

        // make a check for both keys. This will fail since the limit was
        // reached for one key. The counter should not be created for the
        // other key.
        assert_limited(
            limiter.check_and_increment(&[&check_missing, &check_limited]),
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
        assert_not_limited(limiter.check_and_increment(&[&check_key_a]));
        assert_not_limited(limiter.check_and_increment(&[&check_key_b]));

        // confirm that counters now exist
        assert_eq!(limiter.count_for_key(&key_a), Some(1));
        assert_eq!(limiter.count_for_key(&key_b), Some(1));

        // call reset to remove keys
        limiter.reset();

        // confirm that there are no counters for the keys
        assert_eq!(limiter.count_for_key(&key_a), None);
        assert_eq!(limiter.count_for_key(&key_b), None);

        // trigger checks so that the counters are created again
        assert_not_limited(limiter.check_and_increment(&[&check_key_a]));
        assert_not_limited(limiter.check_and_increment(&[&check_key_b]));

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
        assert_not_limited(limiter.check_and_increment_at(&[&check], now));

        // verify that the following check within the same window is limited
        assert_limited(
            limiter.check_and_increment_at(&[&check], now),
            &[key.clone()],
            None,
        );

        // verify that a check is limited if made just before window expires
        assert_limited(
            limiter.check_and_increment_at(
                &[&check],
                now + window - Duration::from_nanos(1),
            ),
            &[key.clone()],
            None,
        );

        // verify that the following check after the window expires is allowed
        assert_not_limited(
            limiter.check_and_increment_at(&[&check], now + window),
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

        let checks = &[&check_key_a, &check_key_b, &check_key_c];

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

    #[test]
    fn policy_check_for_matching_endpoint_and_method_returns_check() {
        let policy = RateLimitPolicy::new(
            "test_policy",
            vec![
                MatchPredicate::Endpoint {
                    any_of: vec!["some_endpoint".to_string()],
                },
                MatchPredicate::HttpMethod { any_of: vec![http::Method::GET] },
            ],
            RateLimitQuota::new(123, Duration::from_secs(60)),
            vec![
                RateLimitKeyPart::Literal("endpoint".to_string()),
                RateLimitKeyPart::Endpoint,
                RateLimitKeyPart::HttpMethod,
            ],
        );

        let ctx =
            RateLimitRequestContext::new("some_endpoint", http::Method::GET);

        let check = policy.check_for(&ctx).expect("policy should match");

        assert_eq!(check.key, RateLimitKey::new("endpoint:some_endpoint:GET"));
        assert_eq!(check.limit, 123);
        assert_eq!(check.window, Duration::from_secs(60));

        // mismatch on endpoint
        let ctx = RateLimitRequestContext::new(
            "some_other_endpoint",
            http::Method::GET,
        );

        assert!(policy.check_for(&ctx).is_none());

        // mismatch on method
        let ctx =
            RateLimitRequestContext::new("some_endpoint", http::Method::POST);

        assert!(policy.check_for(&ctx).is_none());
    }

    #[test]
    fn policy_check_for_endpoint_any_of_matches_any_listed_endpoint() {
        let policy = RateLimitPolicy::new(
            "test_policy",
            vec![MatchPredicate::Endpoint {
                any_of: vec![
                    "some_endpoint_1".to_string(),
                    "some_endpoint_2".to_string(),
                ],
            }],
            RateLimitQuota::new(10, Duration::from_secs(60)),
            vec![RateLimitKeyPart::Endpoint],
        );

        let ctx =
            RateLimitRequestContext::new("some_endpoint_2", http::Method::GET);

        assert!(policy.check_for(&ctx).is_some());
    }

    #[test]
    fn policy_check_for_global_matches_any_request() {
        let policy = RateLimitPolicy::new(
            "global-policy",
            vec![MatchPredicate::Global],
            RateLimitQuota::new(10, Duration::from_secs(60)),
            vec![RateLimitKeyPart::Literal("global".to_string())],
        );

        let ctx =
            RateLimitRequestContext::new("some_endpoint_1", http::Method::GET);

        assert!(policy.check_for(&ctx).is_some());

        let ctx =
            RateLimitRequestContext::new("some_endpoint_2", http::Method::POST);

        assert!(policy.check_for(&ctx).is_some());
    }

    #[test]
    fn db_rate_limit_policy_compiles_to_runtime_policy() {
        // create some db policy model instance
        let db_policy = db_policy(
            "test-policy",
            123,
            60,
            json!([
                {
                    "type": "endpoint",
                    "any_of": ["some_endpoint"],
                },
                {
                    "type": "http_method",
                    "any_of": ["GET"],
                },
            ]),
            json!([
                {
                    "type": "literal",
                    "value": "endpoint",
                },
                {
                    "type": "endpoint",
                },
                {
                    "type": "http_method",
                },
            ]),
        );

        let policy = RateLimitPolicy::try_from(&db_policy).unwrap();

        assert_eq!(
            policy,
            RateLimitPolicy::new(
                "test-policy",
                vec![
                    MatchPredicate::Endpoint {
                        any_of: vec!["some_endpoint".to_string()],
                    },
                    MatchPredicate::HttpMethod {
                        any_of: vec![http::Method::GET],
                    },
                ],
                RateLimitQuota::new(123, Duration::from_secs(60)),
                vec![
                    RateLimitKeyPart::Literal("endpoint".to_string()),
                    RateLimitKeyPart::Endpoint,
                    RateLimitKeyPart::HttpMethod,
                ],
            )
        );
    }

    #[test]
    fn db_rate_limit_policy_compile_rejects_invalid_matcher_json() {
        // create an invalid policy which doesn't have the any_of field for the
        // endpoint variant
        let db_policy = db_policy(
            "test-policy",
            1,
            60,
            json!([
                {
                    "type": "endpoint",
                    "not_any_of": ["some_endpoint"],
                },
            ]),
            json!([
                {
                    "type": "endpoint",
                },
            ]),
        );

        let error = compile_error(&db_policy);
        assert_error_chain_contains(
            &error,
            "couldn't deserialize matchers for rate limit policy test-policy",
        );
    }

    #[test]
    fn db_rate_limit_policy_compile_rejects_invalid_http_method() {
        // create an invalid policy which has an unknown http method
        let db_policy = db_policy(
            "test-policy",
            1,
            60,
            json!([
                {
                    "type": "http_method",
                    "any_of": ["not a method"],
                },
            ]),
            json!([
                {
                    "type": "http_method",
                },
            ]),
        );

        let error = compile_error(&db_policy);
        assert_error_chain_contains(
            &error,
            "couldn't compile rate limit policy test-policy",
        );
        assert_error_chain_contains(&error, "invalid HTTP method");
    }

    #[test]
    fn db_rate_limit_policy_compile_rejects_invalid_quotas() {
        // create an invalid policy which has a 0 quota limit
        let zero_limit = db_policy(
            "zero-limit-policy",
            0,
            60,
            json!([{ "type": "global" }]),
            json!([{ "type": "literal", "value": "global" }]),
        );
        let error = compile_error(&zero_limit);
        assert_error_chain_contains(
            &error,
            "couldn't compile rate limit policy zero-limit-policy",
        );
        assert_error_chain_contains(&error, "invalid quota_limit 0");

        // create an invalid policy which has a 0 window duration
        let zero_window = db_policy(
            "zero-window-policy",
            1,
            0,
            json!([{ "type": "global" }]),
            json!([{ "type": "literal", "value": "global" }]),
        );
        let error = compile_error(&zero_window);
        assert_error_chain_contains(
            &error,
            "couldn't compile rate limit policy zero-window-policy",
        );
        assert_error_chain_contains(&error, "invalid quota_window_seconds 0");
    }

    #[test]
    fn fixed_data_rate_limit_policies_compile_to_runtime_policies() {
        use nexus_db_fixed_data::rate_limit_policy::{
            BUILTIN_RATE_LIMIT_POLICIES, CURRENT_USER_VIEW_POLICY_NAME,
            GLOBAL_POLICY_NAME, USER_BUILTIN_LIST_POLICY_NAME,
        };

        // try to convert builtin policies which we use to seed the db
        let policies = BUILTIN_RATE_LIMIT_POLICIES
            .iter()
            .map(RateLimitPolicy::try_from)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(policies.len(), 3);

        // verify current_user_view policy
        let current_user_view = policies
            .iter()
            .find(|policy| policy.id() == CURRENT_USER_VIEW_POLICY_NAME)
            .unwrap();
        assert_eq!(
            current_user_view,
            &RateLimitPolicy::new(
                CURRENT_USER_VIEW_POLICY_NAME,
                vec![MatchPredicate::Endpoint {
                    any_of: vec!["current_user_view".to_string()],
                }],
                RateLimitQuota::new(2, Duration::from_secs(3600)),
                vec![
                    RateLimitKeyPart::Literal("endpoint".to_string()),
                    RateLimitKeyPart::Endpoint,
                ],
            )
        );

        // verify user_builtin_list policy
        let user_builtin_list = policies
            .iter()
            .find(|policy| policy.id() == USER_BUILTIN_LIST_POLICY_NAME)
            .unwrap();
        assert_eq!(
            user_builtin_list,
            &RateLimitPolicy::new(
                USER_BUILTIN_LIST_POLICY_NAME,
                vec![MatchPredicate::Endpoint {
                    any_of: vec!["user_builtin_list".to_string()],
                }],
                RateLimitQuota::new(2, Duration::from_secs(3600)),
                vec![
                    RateLimitKeyPart::Literal("endpoint".to_string()),
                    RateLimitKeyPart::Endpoint,
                ],
            )
        );

        // verify global policy
        let global = policies
            .iter()
            .find(|policy| policy.id() == GLOBAL_POLICY_NAME)
            .unwrap();
        assert_eq!(
            global,
            &RateLimitPolicy::new(
                GLOBAL_POLICY_NAME,
                vec![MatchPredicate::Global],
                RateLimitQuota::new(2, Duration::from_secs(3600)),
                vec![RateLimitKeyPart::Literal("global".to_string())],
            )
        );
    }

    // helper to create an instance of the db's rate limit policy
    fn db_policy(
        name: &str,
        quota_limit: i64,
        quota_window_seconds: i64,
        matchers: serde_json::Value,
        key_parts: serde_json::Value,
    ) -> db_model::RateLimitPolicy {
        db_model::RateLimitPolicy::new_with_id(
            Uuid::new_v4(),
            IdentityMetadataCreateParams {
                name: name.parse().unwrap(),
                description: format!("test policy {name}"),
            },
            true,
            quota_limit,
            quota_window_seconds,
            matchers,
            key_parts,
        )
    }

    // helper for asserting that compiling a rate limit policy errors out
    fn compile_error(policy: &db_model::RateLimitPolicy) -> anyhow::Error {
        match RateLimitPolicy::try_from(policy) {
            Ok(_) => panic!("policy unexpectedly compiled"),
            Err(error) => error,
        }
    }

    // helper to assert that a specific error was raised
    fn assert_error_chain_contains(error: &anyhow::Error, needle: &str) {
        let chain = error.chain().map(ToString::to_string).collect::<Vec<_>>();
        assert!(
            chain.iter().any(|cause| cause.contains(needle)),
            "error chain {chain:#?} did not contain {needle:?}",
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
