// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::metrics_querier::{MetricsNotYet, MetricsQuerier};
use http::StatusCode;
use nexus_test_utils::http_testing::AuthnMode;
use nexus_test_utils::http_testing::NexusRequest;
use nexus_test_utils::http_testing::{RequestBuilder, TestResponse};
use nexus_test_utils::wait_for_producer;
use nexus_test_utils_macros::nexus_test;
use nexus_types::external_api::oxql;
use oxql_types::point::ValueArray;

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

#[nexus_test]
async fn test_rate_limiting_metrics_are_emitted(
    cptestctx: &ControlPlaneTestContext,
) {
    cptestctx.server.server_context().set_rate_limiting_enabled(true);

    wait_for_producer(
        &cptestctx.oximeter,
        cptestctx.server.server_context().nexus.id(),
    )
    .await;

    let client = &cptestctx.external_client;

    // make two requests which are not limited
    assert_eq!(request(client, "/v1/me").await.status, StatusCode::OK);
    assert_eq!(request(client, "/v1/me").await.status, StatusCode::OK);

    // make one which is limited
    let response = request(client, "/v1/me").await;
    assert_rate_limited(&response);

    // verify that metrics were emitted for endpoint policy
    assert_rate_limited_request_count(
        cptestctx,
        "current_user_view-policy",
        "current_user_view",
        1,
    )
    .await;

    // verify that metrics were emitted for global policy
    assert_rate_limited_request_count(
        cptestctx,
        "global-policy",
        "current_user_view",
        1,
    )
    .await;
}

// helper for making requests, to make tests easier to read
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

// helper for asserting the number of rate limited requests for a sepacific
// policy and endpoint from the the http_service:rate_limited_request metric
async fn assert_rate_limited_request_count(
    cptestctx: &ControlPlaneTestContext,
    policy_id: &'static str,
    operation_id: &'static str,
    expected_count: i64,
) {
    // construct the query
    let query = format!(
        "get http_service:rate_limited_request \
         | filter timestamp > @now() - 1h \
         | filter policy_id == '{policy_id}' \
         | filter operation_id == '{operation_id}'"
    );

    // execute the query
    MetricsQuerier::new(cptestctx)
        .system_timeseries_query_until(&query, |tables| {
            // we're querying a single table, so we can just use .first here
            let table = tables
                .first()
                .ok_or_else(|| MetricsNotYet::new("table is missing"))?;

            // extract the limited request count from the table
            let count = rate_limited_request_count_from_table(table)?;

            // wait for a bit longer if the counts are not the same
            if count != expected_count {
                return Err(MetricsNotYet::new(format!(
                    "waiting for rate limiting samples"
                )));
            }

            Ok(())
        })
        .await;
}

// helper to get the sum value from the timeseries. even though it's emitted as a
// cumulative sum, the results are returned as deltas when querying via OxQL, if i
// understand correctly
fn rate_limited_request_count_from_table(
    table: &oxql::OxqlTable,
) -> Result<i64, MetricsNotYet> {
    let Some(timeseries) = table.timeseries.first() else {
        return Ok(0);
    };

    // take the sum to get the total count since timeseries is returned as deltas
    match timeseries.points.values(0) {
        Some(ValueArray::Integer(vals)) => {
            Ok(vals.iter().filter_map(|&v| v).sum::<i64>())
        }
        other => panic!("expected integer values, found {other:?}"),
    }
}
