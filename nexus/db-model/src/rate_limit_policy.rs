// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::Generation;
use db_macros::Resource;
use nexus_db_schema::schema::{
    rate_limit_policy, rate_limit_policy_generation,
};
use omicron_common::api::external::IdentityMetadataCreateParams;
use uuid::Uuid;

#[derive(Queryable, Clone, Debug, Selectable, Insertable)]
#[diesel(table_name = rate_limit_policy_generation)]
pub struct RateLimitPolicyGeneration {
    pub singleton: bool,
    pub generation: Generation,
}

/// Configuration for a rate limit policy.
#[derive(Queryable, Clone, Debug, Selectable, Insertable, Resource)]
#[diesel(table_name = rate_limit_policy)]
pub struct RateLimitPolicy {
    #[diesel(embed)]
    identity: RateLimitPolicyIdentity,

    pub enabled: bool,
    pub quota_limit: i64,
    pub quota_window_seconds: i64,
    pub matchers: serde_json::Value,
    pub key_parts: serde_json::Value,
}

impl RateLimitPolicy {
    pub fn new_with_id(
        id: Uuid,
        identity: IdentityMetadataCreateParams,
        enabled: bool,
        quota_limit: i64,
        quota_window_seconds: i64,
        matchers: serde_json::Value,
        key_parts: serde_json::Value,
    ) -> Self {
        Self {
            identity: RateLimitPolicyIdentity::new(id, identity),
            enabled,
            quota_limit,
            quota_window_seconds,
            matchers,
            key_parts,
        }
    }
}
