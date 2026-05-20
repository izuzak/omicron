// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use http::StatusCode;
use nexus_test_utils::http_testing::AuthnMode;
use nexus_test_utils::http_testing::NexusRequest;
use nexus_test_utils::http_testing::{RequestBuilder, TestResponse};
use nexus_test_utils_macros::nexus_test;

type ControlPlaneTestContext =
    nexus_test_utils::ControlPlaneTestContext<omicron_nexus::Server>;

#[nexus_test]
async fn test_rate_limiting_enabled(cptestctx: &ControlPlaneTestContext) {
    cptestctx.server.server_context().set_rate_limiting_enabled(true);

    let client = &cptestctx.external_client;

    let response = request(client, "/v1/me").await;
    assert_eq!(response.status, StatusCode::OK);

    let response = request(client, "/v1/me").await;
    assert_eq!(response.status, StatusCode::OK);

    let response = request(client, "/v1/me").await;
    assert_rate_limited(&response);

    let response = request(client, "/v1/system/users-builtin").await;
    assert_rate_limited(&response);
}

#[nexus_test]
async fn test_rate_limiting_disabled_by_default(
    cptestctx: &ControlPlaneTestContext,
) {
    let client = &cptestctx.external_client;

    let response = request(client, "/v1/me").await;
    assert_eq!(response.status, StatusCode::OK);

    let response = request(client, "/v1/me").await;
    assert_eq!(response.status, StatusCode::OK);

    // this would fail if rate limiting were enabled
    let response = request(client, "/v1/me").await;
    assert_eq!(response.status, StatusCode::OK);

    // this would fail if rate limiting were enabled
    let response = request(client, "/v1/system/users-builtin").await;
    assert_eq!(response.status, StatusCode::OK);
}

async fn request(
    client: &dropshot::test_util::ClientTestContext,
    path: &str,
) -> TestResponse {
    NexusRequest::new(RequestBuilder::new(client, http::Method::GET, path))
        .authn_as(AuthnMode::PrivilegedUser)
        .execute()
        .await
        .expect("request failed")
}

fn assert_has_retry_after_header(response: &TestResponse) {
    let retry_after = response
        .headers
        .get(http::header::RETRY_AFTER)
        .expect("expected Retry-After header");
    let retry_after = retry_after
        .to_str()
        .expect("Retry-After header should be valid ascii string");
    let retry_after = retry_after
        .parse::<u64>()
        .expect("Retry-After header should be an unsigned integer");

    assert!(retry_after > 0, "Retry-After header should be positive");
}

fn assert_rate_limited(response: &TestResponse) {
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_has_retry_after_header(&response);
}
