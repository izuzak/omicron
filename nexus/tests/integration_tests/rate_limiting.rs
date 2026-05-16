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
async fn test_make_single_request_to_two_endpoints(
    cptestctx: &ControlPlaneTestContext,
) {
    let client = &cptestctx.external_client;

    let response = request(client, "/v1/me").await;
    assert_eq!(response.status, StatusCode::OK);

    let response = request(client, "/v1/me").await;
    assert_eq!(response.status, StatusCode::OK);

    let response = request(client, "/v1/me").await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);

    let response = request(client, "/v1/system/users-builtin").await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
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
