// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use api_identity::ObjectIdentity;
use omicron_common::api::external::{IdentityMetadata, ObjectIdentity};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// These are the "public" structures used for representing policies in the API
// responses. They pretty much mirror the internal structures today.

#[derive(ObjectIdentity, Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RateLimitPolicy {
    #[serde(flatten)]
    pub identity: IdentityMetadata,
    pub enabled: bool,
    pub matchers: Vec<RateLimitMatcher>,
    pub quota: RateLimitQuota,
    pub key_parts: Vec<RateLimitKeyPart>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RateLimitQuota {
    pub limit: u64,
    pub window_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RateLimitMatcher {
    Endpoint { any_of: Vec<String> },
    HttpMethod { any_of: Vec<String> },
    Global,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RateLimitKeyPart {
    Literal { value: String },
    Endpoint,
    HttpMethod,
}
