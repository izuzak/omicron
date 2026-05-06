// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use dropshot::test_util::ClientTestContext;
use http::{StatusCode, header, method::Method};
use nexus_test_utils::http_testing::{AuthnMode, NexusRequest, RequestBuilder};
use nexus_test_utils::resource_helpers::grant_iam;
use nexus_test_utils::resource_helpers::test_params;
use nexus_test_utils::resource_helpers::{create_local_user, create_silo};
use nexus_test_utils_macros::nexus_test;
use nexus_types::external_api::policy::SiloRole;
use nexus_types::external_api::silo;
use nexus_types::external_api::silo::SiloIdentityMode;
use nexus_types::external_api::user;
use omicron_common::api::external::{Name, UserId};
use omicron_passwords::MIN_EXPECTED_PASSWORD_VERIFY_TIME;
use std::str::FromStr;
use std::time::Duration;

type ControlPlaneTestContext =
    nexus_test_utils::ControlPlaneTestContext<omicron_nexus::Server>;

// TODO-coverage verify that deleting a Silo deletes all the users and their
// password hashes

#[derive(Clone)]
struct TimedLoginCase {
    name: &'static str,
    username: UserId,
    password: &'static str,
    expected_outcome: TimedLoginOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimedLoginOutcome {
    Failure,
    Success,
}

impl TimedLoginOutcome {
    fn expected_status(self) -> StatusCode {
        match self {
            TimedLoginOutcome::Failure => StatusCode::UNAUTHORIZED,
            TimedLoginOutcome::Success => StatusCode::NO_CONTENT,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct DurationStats {
    n: usize,
    mean_seconds: f64,
    variance_seconds_squared: f64,
    min: Duration,
    max: Duration,
}

// This test attempts to detect timing differences between different cases of
// login attempts: nonexistent user, user with no password set, successful
// login, and user with a password set that doesn't match what's provided. It
// is more of a regression test for large timing differences between these
// cases than a proof that the login flow has no timing side channels.
//
// We use the dudect-style of timing-leak detection:
//   1. Collect timing samples for different login attempt cases. Since login
//      attempts are expensive, we keep the sample count modest. This keeps the
//      test runtime reasonable, while still collecting enough samples to catch
//      large regressions.
//   2. Compute Welch's t-statistic between cases. For each pair of login
//      cases, we compute roughly:
//      t = (mean_a - mean_b) / sqrt(var_a / n_a + var_b / n_b)
//   3. Treat a large absolute t-statistic as evidence of timing leakage, i.e.
//      two timing distributions are distinguishable enough to be suspicious.
//
// We use Welch's t-statistic rather than the Student's t-statistic because
// Welch's test does not assume equal variance between the two sample groups.
// Different login outcomes may have different variance even if their average
// times are similar.
//
// See these for more details on dudect and Welch's t-test:
//   https://github.com/oreparaz/dudect
//   https://en.wikipedia.org/wiki/Welch%27s_t-test

#[nexus_test]
async fn test_local_users(cptestctx: &ControlPlaneTestContext) {
    let client = &cptestctx.external_client;

    let silo_name = Name::from_str("test-silo").unwrap();
    let silo = create_silo(
        client,
        silo_name.as_str(),
        true,
        SiloIdentityMode::LocalOnly,
    )
    .await;
    test_local_user_basic(client, &silo).await;
    test_local_user_with_no_initial_password(client, &silo).await;
    NexusRequest::object_delete(
        client,
        &format!("/v1/system/silos/{}", silo_name),
    )
    .authn_as(AuthnMode::PrivilegedUser)
    .execute()
    .await
    .unwrap();
}

#[nexus_test]
async fn test_local_login_timing(cptestctx: &ControlPlaneTestContext) {
    const LOGIN_TIMING_WARMUP_ROUNDS: usize = 1;
    const LOGIN_TIMING_SAMPLE_COUNT: usize = 10;

    let client = &cptestctx.external_client;

    let silo_name = Name::from_str("timing-silo").unwrap();
    let silo = create_silo(
        client,
        silo_name.as_str(),
        true,
        SiloIdentityMode::LocalOnly,
    )
    .await;

    // Create the two users to exercise the "has password hash" and "has no
    // password hash" paths.
    let password_user = UserId::from_str("timing-password-user").unwrap();
    let password = "banana";
    create_local_user(
        client,
        &silo,
        &password_user,
        test_params::UserPassword::Password(password.to_string()),
    )
    .await;

    let no_password_user = UserId::from_str("timing-no-password-user").unwrap();
    create_local_user(
        client,
        &silo,
        &no_password_user,
        test_params::UserPassword::LoginDisallowed,
    )
    .await;

    // These are the login cases we want to compare.
    let cases = [
        TimedLoginCase {
            name: "nonexistent user",
            username: UserId::from_str("timing-nonexistent-user").unwrap(),
            password: "pineapple",
            expected_outcome: TimedLoginOutcome::Failure,
        },
        TimedLoginCase {
            name: "user with no password",
            username: no_password_user,
            password: "avocado",
            expected_outcome: TimedLoginOutcome::Failure,
        },
        TimedLoginCase {
            name: "successful login",
            username: password_user.clone(),
            password,
            expected_outcome: TimedLoginOutcome::Success,
        },
        TimedLoginCase {
            name: "wrong password",
            username: password_user,
            password: "mango",
            expected_outcome: TimedLoginOutcome::Failure,
        },
    ];

    // Discard a small warmup set so setup effects don't dominate the
    // statistical samples.
    for _ in 0..LOGIN_TIMING_WARMUP_ROUNDS {
        for case in &cases {
            let _ = timed_login_attempt(client, &silo_name, case).await;
        }
    }

    let mut samples = cases
        .iter()
        .map(|_| Vec::with_capacity(LOGIN_TIMING_SAMPLE_COUNT))
        .collect::<Vec<_>>();

    // Collect one sample for every case in each round, rather than collecting
    // all samples for one case before moving to the next. Also rotate which
    // case starts each round: A/B/C/D, then B/C/D/A, and so on. The first
    // interleaving spreads slow or fast periods on the test machine across all
    // cases while the rotation avoids assigning any first/last-in-round
    // effects to the same case every time.
    for round in 0..LOGIN_TIMING_SAMPLE_COUNT {
        for offset in 0..cases.len() {
            let case_index = (round + offset) % cases.len();
            let elapsed =
                timed_login_attempt(client, &silo_name, &cases[case_index])
                    .await;
            samples[case_index].push(elapsed);
        }
    }

    assert_login_timing_samples(&cases, &samples);

    NexusRequest::object_delete(
        client,
        &format!("/v1/system/silos/{}", silo_name),
    )
    .authn_as(AuthnMode::PrivilegedUser)
    .execute()
    .await
    .unwrap();
}

// Exercise the timing assertion's failure path with deterministic synthetic
// samples. The two sample buckets have a large mean difference relative to
// their variance, so the Welch t-statistic exceeds the configured threshold.
// As a result, it's expected that the assertion fails and the test panics.
#[test]
#[should_panic(
    expected = "login timing samples are statistically distinguishable"
)]
fn test_login_timing_samples_reject_synthetic_timing_gap() {
    let cases = [
        TimedLoginCase {
            name: "synthetic fast login",
            username: UserId::from_str("synthetic-fast-login").unwrap(),
            password: "broccoli",
            expected_outcome: TimedLoginOutcome::Failure,
        },
        TimedLoginCase {
            name: "synthetic slow login",
            username: UserId::from_str("synthetic-slow-login").unwrap(),
            password: "coconut",
            expected_outcome: TimedLoginOutcome::Failure,
        },
    ];

    let samples = vec![
        vec![100, 101, 98, 102, 99, 101, 100, 103, 97, 101],
        vec![200, 201, 198, 202, 199, 201, 200, 203, 197, 201],
    ]
    .into_iter()
    .map(|case_samples| {
        case_samples.into_iter().map(Duration::from_millis).collect::<Vec<_>>()
    })
    .collect::<Vec<_>>();

    assert_login_timing_samples(&cases, &samples);
}

// Two unlikely inputs to exercise the branches of welch_t_statistic where
// case variances are zero:
//   * Different means with zero variance: the cases are perfectly
//     distinguishable, so t should be signed infinity.
//   * Identical cases: both mean difference and variances are zero,
//     so t should be 0.0.
#[test]
fn test_welch_t_statistic_handles_zero_variance_timing_case() {
    let fast_stats = duration_stats(&[Duration::from_millis(100); 10]);
    let slow_stats = duration_stats(&[Duration::from_millis(200); 10]);

    let t = welch_t_statistic(&fast_stats, &slow_stats);
    assert!(t.is_infinite());
    assert!(t.is_sign_negative());

    assert_eq!(welch_t_statistic(&fast_stats, &fast_stats), 0.0);
}

async fn test_local_user_basic(client: &ClientTestContext, silo: &silo::Silo) {
    let silo_name = &silo.identity.name;

    // First, try logging in with a non-existent user.  This naturally should
    // fail.  It should also take as long as it would take for a valid user.
    // The timing is verified in expect_login_failure().
    expect_login_failure(
        client,
        &silo_name,
        UserId::from_str("bigfoot").unwrap(),
        "ahh".to_string(),
    )
    .await;

    // Create a test user with a known password.
    let test_user = UserId::from_str("abe-simpson").unwrap();
    let test_password = "let me in you idiot!";

    let created_user = create_local_user(
        client,
        silo,
        &test_user,
        test_params::UserPassword::Password(test_password.to_string()),
    )
    .await;

    // Try to log in with a bogus password.
    expect_login_failure(
        client,
        &silo_name,
        test_user.clone(),
        "something else".to_string(),
    )
    .await;

    // Then log in with the right password and use the session token to do
    // something.
    let session_token = expect_login_success(
        client,
        &silo_name,
        test_user.clone(),
        test_password.to_string(),
    )
    .await;
    let found_user = expect_session_valid(client, &session_token).await;
    assert_eq!(created_user, found_user.user);

    // While we're still logged in, change the password.
    let test_password2 = "as was the style at the time";
    let user_password_url = format!(
        "/v1/system/identity-providers/local/users/{}/set-password?silo={}",
        created_user.id, silo_name
    );
    NexusRequest::new(
        RequestBuilder::new(client, Method::POST, &user_password_url)
            .expect_status(Some(StatusCode::NO_CONTENT))
            .body(Some(&test_params::UserPassword::Password(
                test_password2.to_string(),
            ))),
    )
    .authn_as(AuthnMode::Session(session_token.to_string()))
    .execute()
    .await
    .unwrap();

    // The old password should no longer work.
    expect_login_failure(
        client,
        &silo_name,
        test_user.clone(),
        test_password.to_string(),
    )
    .await;

    // We should be able to login separately with the new password.
    let session_token2 = expect_login_success(
        client,
        &silo_name,
        test_user.clone(),
        test_password2.to_string(),
    )
    .await;

    // At this point, both session tokens should be valid.
    expect_session_valid(client, &session_token).await;
    expect_session_valid(client, &session_token2).await;

    // Log out of the first session.
    logout_session(client, &session_token).await;

    // The first session token should not be valid any more.
    expect_session_invalid(client, &session_token).await;

    // But the second session token should still be valid.
    expect_session_valid(client, &session_token2).await;

    // Now, let's create an admin user and verify that they can change this
    // user's password.
    let admin_user = UserId::from_str("comic-book-guy").unwrap();
    let admin_password = "toodle-ooh";
    let admin_user_obj = create_local_user(
        client,
        silo,
        &admin_user,
        test_params::UserPassword::Password(admin_password.to_string()),
    )
    .await;
    let admin_password_url = format!(
        "/v1/system/identity-providers/local/users/{}/set-password?silo={}",
        admin_user_obj.id, silo_name
    );

    let silo_url = format!("/v1/system/silos/{}", silo_name);
    grant_iam(
        client,
        &silo_url,
        SiloRole::Admin,
        admin_user_obj.id,
        AuthnMode::PrivilegedUser,
    )
    .await;

    let admin_session = expect_login_success(
        client,
        &silo_name,
        admin_user.clone(),
        admin_password.to_string(),
    )
    .await;

    let hijacked_password = "sarcasm detector";
    NexusRequest::new(
        RequestBuilder::new(client, Method::POST, &user_password_url)
            .expect_status(Some(StatusCode::NO_CONTENT))
            .body(Some(&test_params::UserPassword::Password(
                hijacked_password.to_string(),
            ))),
    )
    .authn_as(AuthnMode::Session(admin_session.to_string()))
    .execute()
    .await
    .unwrap();

    // Just to be clear, we modified the test user's password.
    let _ = expect_login_success(
        client,
        &silo_name,
        test_user.clone(),
        hijacked_password.to_string(),
    )
    .await;
    expect_login_failure(
        client,
        &silo_name,
        test_user.clone(),
        test_password2.to_string(),
    )
    .await;

    // And we did not modify the admin user's password.
    let _ = expect_login_success(
        client,
        &silo_name,
        admin_user.clone(),
        admin_password.to_string(),
    )
    .await;
    expect_login_failure(
        client,
        &silo_name,
        admin_user.clone(),
        hijacked_password.to_string(),
    )
    .await;

    // The admin can also invalidate the user's password.
    NexusRequest::new(
        RequestBuilder::new(client, Method::POST, &user_password_url)
            .expect_status(Some(StatusCode::NO_CONTENT))
            .body(Some(&test_params::UserPassword::LoginDisallowed)),
    )
    .authn_as(AuthnMode::Session(admin_session.to_string()))
    .execute()
    .await
    .unwrap();
    expect_login_failure(
        client,
        &silo_name,
        test_user.clone(),
        hijacked_password.to_string(),
    )
    .await;
    // And we did not modify the admin user's password.
    let _ = expect_login_success(
        client,
        &silo_name,
        admin_user.clone(),
        admin_password.to_string(),
    )
    .await;
    expect_login_failure(
        client,
        &silo_name,
        admin_user.clone(),
        hijacked_password.to_string(),
    )
    .await;

    // But the ordinary user can neither set or invalidate the admin user's
    // password.  (i.e., users cannot reset each other's passwords unless
    // they're administrators).
    expect_session_valid(client, &session_token2).await;
    NexusRequest::expect_failure_with_body(
        client,
        StatusCode::FORBIDDEN,
        Method::POST,
        &admin_password_url,
        &test_params::UserPassword::Password(test_password.to_string()),
    )
    .authn_as(AuthnMode::Session(session_token2.clone()))
    .execute()
    .await
    .unwrap();

    NexusRequest::expect_failure_with_body(
        client,
        StatusCode::FORBIDDEN,
        Method::POST,
        &admin_password_url,
        &test_params::UserPassword::LoginDisallowed,
    )
    .authn_as(AuthnMode::Session(session_token2.clone()))
    .execute()
    .await
    .unwrap();
}

async fn test_local_user_with_no_initial_password(
    client: &ClientTestContext,
    silo: &silo::Silo,
) {
    let silo_name = &silo.identity.name;

    // Create a user with no initial password.
    let test_user = UserId::from_str("steven-falken").unwrap();
    let created_user = create_local_user(
        client,
        silo,
        &test_user,
        test_params::UserPassword::LoginDisallowed,
    )
    .await;

    // Logging in should not work.  (What password would we use, anyway?)
    expect_login_failure(client, &silo_name, test_user.clone(), "".to_string())
        .await;

    // Now, set a password.
    let test_password2 = "joshua";
    let user_password_url = format!(
        "/v1/system/identity-providers/local/users/{}/set-password?silo={}",
        created_user.id, silo_name,
    );
    NexusRequest::new(
        RequestBuilder::new(client, Method::POST, &user_password_url)
            .expect_status(Some(StatusCode::NO_CONTENT))
            .body(Some(&test_params::UserPassword::Password(
                test_password2.to_string(),
            ))),
    )
    .authn_as(AuthnMode::PrivilegedUser)
    .execute()
    .await
    .unwrap();

    // Now, we should be able to log in and do things.
    let session_token = expect_login_success(
        client,
        &silo_name,
        test_user.clone(),
        test_password2.to_string(),
    )
    .await;
    let found_user = expect_session_valid(client, &session_token).await;
    assert_eq!(created_user, found_user.user);
}

async fn expect_session_valid(
    client: &ClientTestContext,
    session_token: &str,
) -> user::CurrentUser {
    NexusRequest::object_get(client, "/v1/me")
        .authn_as(AuthnMode::Session(session_token.to_string()))
        .execute_and_parse_unwrap::<user::CurrentUser>()
        .await
}

async fn expect_session_invalid(
    client: &ClientTestContext,
    session_token: &str,
) {
    NexusRequest::expect_failure(
        client,
        StatusCode::UNAUTHORIZED,
        Method::GET,
        "/v1/me",
    )
    .authn_as(AuthnMode::Session(session_token.to_string()))
    .execute()
    .await
    .expect(
        "expected request failure due to invalid session token, found success",
    );
}

async fn expect_login_failure(
    client: &ClientTestContext,
    silo_name: &Name,
    username: UserId,
    password: String,
) {
    let start = std::time::Instant::now();
    let login_url = format!("/v1/login/{}/local", silo_name);
    let error: dropshot::HttpErrorResponseBody =
        NexusRequest::expect_failure_with_body(
            client,
            StatusCode::UNAUTHORIZED,
            Method::POST,
            &login_url,
            &test_params::UsernamePasswordCredentials { username, password },
        )
        .execute()
        .await
        .expect("expected login failure, got success")
        .parsed_body()
        .expect("unexpected error format from login failure");
    let elapsed = start.elapsed();

    assert_eq!(error.message, "credentials missing or invalid");

    assert_minimum_password_verify_time(elapsed, "failed login attempt");
}

async fn expect_login_success(
    client: &ClientTestContext,
    silo_name: &Name,
    username: UserId,
    password: String,
) -> String {
    let start = std::time::Instant::now();
    let login_url = format!("/v1/login/{}/local", silo_name);
    let response = RequestBuilder::new(client, Method::POST, &login_url)
        .body(Some(&test_params::UsernamePasswordCredentials {
            username,
            password,
        }))
        .expect_status(Some(StatusCode::NO_CONTENT))
        .execute()
        .await
        .expect("expected successful login, but it failed");
    let elapsed = start.elapsed();
    let session_token = session_token_from_headers(&response.headers);

    // It's not clear how a successful login could ever take less than the
    // minimum verification time, but we verify it here anyway.  (If we fail
    // here, it's possible that our hash parameters have gotten too weak for the
    // current hardware.  See the similar test in the omicron_passwords module.)
    assert_minimum_password_verify_time(elapsed, "successful login");

    session_token
}

async fn timed_login_attempt(
    client: &ClientTestContext,
    silo_name: &Name,
    case: &TimedLoginCase,
) -> Duration {
    let login_url = format!("/v1/login/{}/local", silo_name);
    let credentials = test_params::UsernamePasswordCredentials {
        username: case.username.clone(),
        password: case.password.to_string(),
    };

    let start = std::time::Instant::now();
    let response = RequestBuilder::new(client, Method::POST, &login_url)
        .body(Some(&credentials))
        .expect_status(Some(case.expected_outcome.expected_status()))
        .execute()
        .await
        .unwrap_or_else(|error| {
            panic!("login timing case {:?} failed: {:#}", case.name, error)
        });
    let elapsed = start.elapsed();

    match case.expected_outcome {
        TimedLoginOutcome::Failure => {
            let error: dropshot::HttpErrorResponseBody = response
                .parsed_body()
                .expect("unexpected error format from login failure");
            assert_eq!(error.message, "credentials missing or invalid");
        }
        TimedLoginOutcome::Success => {
            let session_token = session_token_from_headers(&response.headers);
            logout_session(client, &session_token).await;
        }
    }

    assert_minimum_password_verify_time(elapsed, case.name);
    elapsed
}

// Log out a session token via the external API.
async fn logout_session(client: &ClientTestContext, session_token: &str) {
    NexusRequest::new(
        RequestBuilder::new(client, Method::POST, "/v1/logout")
            .expect_status(Some(StatusCode::NO_CONTENT)),
    )
    .authn_as(AuthnMode::Session(session_token.to_string()))
    .execute()
    .await
    .expect("failed to log out");
}

fn session_token_from_headers(headers: &http::HeaderMap) -> String {
    let cookie_header = headers
        .get(header::SET_COOKIE)
        .expect("session cookie: missing header")
        .to_str()
        .expect("session cookie: header value was not a string");
    let (token_cookie, rest) = cookie_header
        .split_once("; ")
        .expect("session cookie: bad cookie header value (missing semicolon)");
    assert!(token_cookie.starts_with("session="));
    assert_eq!(rest, "Path=/; HttpOnly; SameSite=Lax; Max-Age=86400");
    let (_, session_token) = token_cookie
        .split_once('=')
        .expect("session cookie: bad cookie header value (missing 'session=')");

    session_token.to_string()
}

fn assert_minimum_password_verify_time(elapsed: Duration, description: &str) {
    // Check that login attempts take at least as long as the minimum
    // verification time.  Otherwise, we might have a failure path that exposes a
    // timing attack.  (For example, suppose we returned quickly when you
    // attempted to log in as a user that does not exist.  An attacker could
    // learn whether or not a specific user exists based on how long it took for
    // a login attempt to fail.)
    if elapsed < MIN_EXPECTED_PASSWORD_VERIFY_TIME {
        panic!(
            "{} unexpectedly took less time ({:?}) than \
             minimum password verification time ({:?})",
            description, elapsed, MIN_EXPECTED_PASSWORD_VERIFY_TIME
        );
    }
}

// Compute Welch's t-statistic between samples for every pair of login cases
// and compare to threshold.
fn assert_login_timing_samples(
    cases: &[TimedLoginCase],
    samples: &[Vec<Duration>],
) {
    // The t-statistic measures how different two timing distributions look,
    // scaled by their noise. Larger |t| means the two cases are more clearly
    // distinguishable while a small |t| means any difference fits inside the
    // noise.
    //
    // We treat |t| > 5 as evidence of a timing leak. The dudect timing-leak
    // detector's README uses the same rule of thumb:
    //   "t values larger than 5 mean there is very likely a timing leakage."
    //   - https://github.com/oreparaz/dudect
    // Five is also a high statistical bar in absolute terms: under the null
    // hypothesis (same distribution), |t| crosses 5 by chance well under one
    // in a million times per comparison.
    const LOGIN_TIMING_WELCH_T_THRESHOLD: f64 = 5.0;

    assert_eq!(
        cases.len(),
        samples.len(),
        "timing cases and sample buckets must have the same length",
    );

    let stats = samples
        .iter()
        .map(|case_samples| duration_stats(case_samples))
        .collect::<Vec<_>>();

    for left in 0..cases.len() {
        for right in (left + 1)..cases.len() {
            let t = welch_t_statistic(&stats[left], &stats[right]);
            if t.abs() > LOGIN_TIMING_WELCH_T_THRESHOLD {
                panic!(
                    "login timing samples are statistically \
                     distinguishable:\n{}",
                    format_timing_comparison(
                        &cases[left],
                        &stats[left],
                        &cases[right],
                        &stats[right],
                        t,
                        LOGIN_TIMING_WELCH_T_THRESHOLD,
                    )
                );
            }
        }
    }
}

// Compute the per-case statistics needed for Welch's t-test.
fn duration_stats(samples: &[Duration]) -> DurationStats {
    assert!(
        samples.len() >= 2,
        "need at least two timing samples for variance"
    );

    let n = samples.len();
    let n_f64 = n as f64;
    let sum = samples.iter().map(|d| d.as_secs_f64()).sum::<f64>();
    let mean_seconds = sum / n_f64;
    let variance_seconds_squared = samples
        .iter()
        .map(|d| (d.as_secs_f64() - mean_seconds).powi(2))
        .sum::<f64>()
        / (n_f64 - 1.0);
    let min = samples.iter().copied().min().unwrap();
    let max = samples.iter().copied().max().unwrap();

    DurationStats { n, mean_seconds, variance_seconds_squared, min, max }
}

// Welch's t-statistic compares the means of two samples while allowing the
// samples to have different variances.
fn welch_t_statistic(left: &DurationStats, right: &DurationStats) -> f64 {
    let variance_per_sample_left =
        left.variance_seconds_squared / left.n as f64;
    let variance_per_sample_right =
        right.variance_seconds_squared / right.n as f64;
    let denominator =
        (variance_per_sample_left + variance_per_sample_right).sqrt();
    let mean_difference = left.mean_seconds - right.mean_seconds;

    // With ~zero variance and different means, the two sample buckets are
    // perfectly distinguishable.
    if denominator <= f64::EPSILON {
        if mean_difference.abs() <= f64::EPSILON {
            0.0
        } else {
            f64::INFINITY.copysign(mean_difference)
        }
    } else {
        mean_difference / denominator
    }
}

// The degrees of freedom are not used for the dudect-style threshold check, but
// including them in failure output makes it easier to reproduce or analyze a
// failing comparison with an exact statistical test.
fn welch_degrees_of_freedom(
    left: &DurationStats,
    right: &DurationStats,
) -> f64 {
    let variance_per_sample_left =
        left.variance_seconds_squared / left.n as f64;
    let variance_per_sample_right =
        right.variance_seconds_squared / right.n as f64;
    let numerator =
        (variance_per_sample_left + variance_per_sample_right).powi(2);
    let denominator = variance_per_sample_left.powi(2) / (left.n as f64 - 1.0)
        + variance_per_sample_right.powi(2) / (right.n as f64 - 1.0);

    if denominator <= f64::EPSILON {
        f64::INFINITY
    } else {
        numerator / denominator
    }
}

// Keep timing-test failures actionable by reporting both the test statistic and
// the raw distribution summaries that produced it.
fn format_timing_comparison(
    left_case: &TimedLoginCase,
    left_stats: &DurationStats,
    right_case: &TimedLoginCase,
    right_stats: &DurationStats,
    t: f64,
    threshold: f64,
) -> String {
    format!(
        concat!(
            "  comparison: {} vs {}\n",
            "  t: {:.3}\n",
            "  degrees of freedom: {:.1}\n",
            "  threshold: {:.1}\n",
            "  {}:\n",
            "{}",
            "  {}:\n",
            "{}",
        ),
        left_case.name,
        right_case.name,
        t,
        welch_degrees_of_freedom(left_stats, right_stats),
        threshold,
        left_case.name,
        format_duration_stats(left_stats),
        right_case.name,
        format_duration_stats(right_stats),
    )
}

// Convert stored second-based statistics back to milliseconds for readable test
// failure output.
fn format_duration_stats(stats: &DurationStats) -> String {
    format!(
        concat!(
            "    n: {}\n",
            "    mean: {:.3} ms\n",
            "    stddev: {:.3} ms\n",
            "    min: {:.3} ms\n",
            "    max: {:.3} ms\n",
        ),
        stats.n,
        stats.mean_seconds * 1000.0,
        stats.variance_seconds_squared.sqrt() * 1000.0,
        stats.min.as_secs_f64() * 1000.0,
        stats.max.as_secs_f64() * 1000.0,
    )
}
