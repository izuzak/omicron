// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Built-in rate limit policies. This mirrors nexus/src/rate_limit_builtin.rs
//! which will be removed later.

use nexus_db_model as model;
use omicron_common::api::external::IdentityMetadataCreateParams;
use serde_json::json;
use std::sync::LazyLock;

pub const CURRENT_USER_VIEW_POLICY_NAME: &str = "current-user-view-policy";
pub const USER_BUILTIN_LIST_POLICY_NAME: &str = "user-builtin-list-policy";
pub const GLOBAL_POLICY_NAME: &str = "global-policy";

const CURRENT_USER_VIEW_OPERATION_ID: &str = "current_user_view";
const USER_BUILTIN_LIST_OPERATION_ID: &str = "user_builtin_list";

const DEFAULT_LIMIT: i64 = 2;
const DEFAULT_WINDOW_SECONDS: i64 = 3600;

// UUID of built-in rate limit policy for current-user-view endpoint
pub static CURRENT_USER_VIEW_POLICY_ID: LazyLock<uuid::Uuid> =
    LazyLock::new(|| {
        "001de000-726c-4000-8000-000000000000"
            .parse()
            .expect("invalid uuid for current-user-view rate-limit policy")
    });

// UUID of built-in rate limit policy for user-builtin-list endpoint
pub static USER_BUILTIN_LIST_POLICY_ID: LazyLock<uuid::Uuid> =
    LazyLock::new(|| {
        "001de000-726c-4000-8000-000000000001"
            .parse()
            .expect("invalid uuid for user-builtin-list rate-limit policy")
    });

// UUID of built-in global rate limit policy
pub static GLOBAL_POLICY_ID: LazyLock<uuid::Uuid> = LazyLock::new(|| {
    "001de000-726c-4000-8000-000000000002"
        .parse()
        .expect("invalid uuid for global rate-limit policy")
});

// Built in rate limit policies
pub static BUILTIN_RATE_LIMIT_POLICIES: LazyLock<Vec<model::RateLimitPolicy>> =
    LazyLock::new(|| {
        vec![
            model::RateLimitPolicy::new_with_id(
                *CURRENT_USER_VIEW_POLICY_ID,
                IdentityMetadataCreateParams {
                    name: CURRENT_USER_VIEW_POLICY_NAME.parse().unwrap(),
                    description: String::from(
                        "Built-in rate-limit policy for current-user view requests",
                    ),
                },
                true,
                DEFAULT_LIMIT,
                DEFAULT_WINDOW_SECONDS,
                json!([
                    {
                        "type": "endpoint",
                        "any_of": [CURRENT_USER_VIEW_OPERATION_ID],
                    }
                ]),
                json!([
                    {
                        "type": "literal",
                        "value": "endpoint",
                    },
                    {
                        "type": "endpoint",
                    }
                ]),
            ),
            model::RateLimitPolicy::new_with_id(
                *USER_BUILTIN_LIST_POLICY_ID,
                IdentityMetadataCreateParams {
                    name: USER_BUILTIN_LIST_POLICY_NAME.parse().unwrap(),
                    description: String::from(
                        "Built-in rate-limit policy for user-builtin list requests",
                    ),
                },
                true,
                DEFAULT_LIMIT,
                DEFAULT_WINDOW_SECONDS,
                json!([
                    {
                        "type": "endpoint",
                        "any_of": [USER_BUILTIN_LIST_OPERATION_ID],
                    }
                ]),
                json!([
                    {
                        "type": "literal",
                        "value": "endpoint",
                    },
                    {
                        "type": "endpoint",
                    }
                ]),
            ),
            model::RateLimitPolicy::new_with_id(
                *GLOBAL_POLICY_ID,
                IdentityMetadataCreateParams {
                    name: GLOBAL_POLICY_NAME.parse().unwrap(),
                    description: String::from(
                        "Built-in global rate-limit policy",
                    ),
                },
                true,
                DEFAULT_LIMIT,
                DEFAULT_WINDOW_SECONDS,
                json!([
                    {
                        "type": "global",
                    }
                ]),
                json!([
                    {
                        "type": "literal",
                        "value": "global",
                    }
                ]),
            ),
        ]
    });
