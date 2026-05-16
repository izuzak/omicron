// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nexus_test_utils_macros::nexus_test;
use nexus_types::external_api::user;
use nexus_test_utils::http_testing::AuthnMode;
use nexus_test_utils::http_testing::NexusRequest;

type ControlPlaneTestContext =
    nexus_test_utils::ControlPlaneTestContext<omicron_nexus::Server>;

#[nexus_test]
async fn test_make_single_request_to_two_endpoints(
    cptestctx: &ControlPlaneTestContext,
) {
    let client = &cptestctx.external_client;

    for _ in 0..3 {
        NexusRequest::object_get(client, "/v1/me")
        .authn_as(AuthnMode::PrivilegedUser)
        .execute_and_parse_unwrap::<user::CurrentUser>()
        .await;

        NexusRequest::object_get(client, "/v1/system/users-builtin")
        .authn_as(AuthnMode::PrivilegedUser)
        .execute_and_parse_unwrap::<dropshot::ResultsPage<user::UserBuiltin>>()
        .await;
    }
}
