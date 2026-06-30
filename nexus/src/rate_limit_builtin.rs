// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::rate_limit::{
    MatchPredicate, RateLimitKeyPart, RateLimitPolicy, RateLimitQuota,
};
use std::sync::LazyLock;
use std::time::Duration;

pub(crate) const CURRENT_USER_VIEW_POLICY_ID: &str = "current_user_view-policy";
pub(crate) const USER_BUILTIN_LIST_POLICY_ID: &str = "user_builtin_list-policy";
pub(crate) const GLOBAL_POLICY_ID: &str = "global-policy";

const CURRENT_USER_VIEW_OPERATION_ID: &str = "current_user_view";
const USER_BUILTIN_LIST_OPERATION_ID: &str = "user_builtin_list";

const DEFAULT_LIMIT: usize = 2;
const DEFAULT_WINDOW: Duration = Duration::from_secs(3600);

// These are built in policies for rate limiting, i.e. policies defined by
// oxide. These are also mirrored in db-fixed-data/src/rate_limit_policy.rs
// and will later be removed once rate limit enforcement starts using policies
// from the database. For now they still exist.
static BUILTIN_RATE_LIMIT_POLICIES: LazyLock<Vec<RateLimitPolicy>> =
    LazyLock::new(|| {
        vec![
            // An example policy for the current_user_view endpoint
            RateLimitPolicy::new(
                CURRENT_USER_VIEW_POLICY_ID,
                vec![MatchPredicate::Endpoint {
                    any_of: vec![CURRENT_USER_VIEW_OPERATION_ID.to_string()],
                }],
                RateLimitQuota::new(DEFAULT_LIMIT, DEFAULT_WINDOW),
                vec![
                    RateLimitKeyPart::Literal("endpoint".to_string()),
                    RateLimitKeyPart::Endpoint,
                ],
            ),
            // An example policy for the user_builtin_list endpoint
            RateLimitPolicy::new(
                USER_BUILTIN_LIST_POLICY_ID,
                vec![MatchPredicate::Endpoint {
                    any_of: vec![USER_BUILTIN_LIST_OPERATION_ID.to_string()],
                }],
                RateLimitQuota::new(DEFAULT_LIMIT, DEFAULT_WINDOW),
                vec![
                    RateLimitKeyPart::Literal("endpoint".to_string()),
                    RateLimitKeyPart::Endpoint,
                ],
            ),
            // An example global policy
            RateLimitPolicy::new(
                GLOBAL_POLICY_ID,
                vec![MatchPredicate::Global],
                RateLimitQuota::new(DEFAULT_LIMIT, DEFAULT_WINDOW),
                vec![RateLimitKeyPart::Literal("global".to_string())],
            ),
        ]
    });

pub(crate) fn builtin_rate_limit_policies() -> &'static [RateLimitPolicy] {
    &BUILTIN_RATE_LIMIT_POLICIES
}
