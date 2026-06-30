# What is this?

This is just a playground for me to explore adding rate limits to Nexus'
external API.

Rate limits are interesting to me in many ways (engineering, customer
experience, supportability) so this is a project/space for me to learn through
exploration in a production system I want get to know better.

The goal (a working rate limiting system) is not the only goal, the goal is
learning. So, I might do things in sub-optimal, roundabout ways for the purpose
of learning and exploration. 

# Current state

I'll try to continuously update this section to list the large pieces I (at least partially) explored/implemented:

- In-memory fixed-window rate limiter
- Support for checking multiple limits for a single request
- Usage of the limiter for the nexus external API
- Rate limiting policies for defining for which request to apply limiting and how 
- Metrics for limited requests

# General notes in no particular order

Sooo, a rate limiter has some notion of:
- the actors making requests (e.g. identity of authenticated user, IP address)
- the resources being targeted by those requests (e.g. HTTP endpoint or exact
  resource)
- the rate limiting policies (e.g. which quotas apply for which actors and
  resource, how quotas refresh, which costs apply to which requests, which rate
  limit algorithms to use)
- the rate limit algorithm (the mechanics of refreshing quotas and deducting
  cost to determine if a request should be limited, e.g. fixed window)

Things that should/could exist:
- configurability (e.g. rate limit policy can be configured, isn't hardcoded)
- internal observability (e.g. metrics, logging)
- runtime overrides (e.g. disable limits for some endpoint or user, change
  quotas)
- debuggability/supportability (e.g. which users are limited or were limited,
  which overrides exist)

Some relevant standards docs/RFCs:
- RFC 6585 - Additional HTTP Status Codes
    - documents the 429 status code
    - https://datatracker.ietf.org/doc/html/rfc6585
- RFC 9110 - HTTP Semantics
    - documents the Retry-After response header
    - https://datatracker.ietf.org/doc/html/rfc9110
- RateLimit header fields for HTTP 
    - documents RateLimit header fields. Expired status.
    - https://datatracker.ietf.org/doc/draft-ietf-httpapi-ratelimit-headers/
    - https://github.com/ietf-wg-httpapi/ratelimit-headers

# Nexus APIs and kinda a log of progress

Here's what I want to try:
1. Create or duplicate a test which makes a single request to API endpoint A and
   endpoint B. Pick some endpoints which don't change state. This should pass.
2. Change the test to make 3 requests to endpoint A and 3 request to endpoint B.
   This should still pass.
3. Implement the simplest possible hardcoded in-memory counter which causes
   requests to be denied with a 429 status after some number of requests are
   processed. 
    - this counter needs to be in some state that survives the request
      lifecycle, i.e. it's attached to the server. 
    - ServerContext might be one such place. Buuuut it's passed around wrapped
      in an Arc, which only gives shared ownership, not mutability.
    - i guess there are a few ways to get that, and after a bit of reading an
      AtomicUsize might be the simplest solution. It seems to give both
      mutability (via interior mutability) and safe access across threads.
    - next question is where to put the "limiting" logic -- should be some code
      that all endpoints use.
    - seems like there's no great place for this, hmmm. No middleware concept in
      dropshot or nexus, or some common path all endpoints take.
    - so, i'll put the "limiting" logic into only those two endpoints i'm
      testing with, and then generalize/abstract later.
4. Okay, there are several things I could explore next. Gonna try improving the
   limiter to support multiple limits via a limiter key.
    - each key has its own limiter state
    - when a request is being processed, determine the list of limiter keys that
      should be checked (this is a part of the policy). For now, the list is
      hardcoded and simple.
    - all limiting keys for a request are checked and the request is allowed
      only if checking all keys succeeds
    - for the initial version, i'll lock the whole container (with rate limiting
      states for all the keys) when checking limits even though i could lock
      only the states for the keys i need to check
    - also, there's a choice here between the std mutex and the tokio mutex.
5. Moving rate limiter into a separate file and adding unit tests
6. Dont create counters when multi-key check fails
    - Also add a way to get the counter value for a unit test for this
7. Return a RateLimitDecision enum instead of a boolean to allow determining for
   which key the limit was reached.
    - Might turn this into a Result?
    - yeah, converted to a Result
8. Ok, I want to finally move in the direction of having rate limiting policies
   by not hardcoding the limit and instead passing it to the check.
    - First step: just replace the hardcoded limit and pass it in as a
      RateLimitCheck.
    - Second step: extract checks for endpoints into a separate method.
    - Third step: extract rate limit checking into a separate method.
9. Making some improvements to the limiter itself so that it's more like a real
   limiter.
    - Add reset and reset_key methods to, well, remove the state for a key.
    - Add the fixed-window bits: window start time and duration.
    - Make it testable via a helper which allows passing in time directly.
    - Return more information when limited so that we can include a retry-after
      header.
10. Make it possible to enable/disable the limiter via config
    - Enabled by default, disabled in tests
    - When disabled, rate limiting is skipped when processing requests
    - And can be enabled/disabled manually, e.g. we enable it in rate limiting
      integration tests
    - Might turn the enabled/disabled bool into a mode in the future, so that we
      can also have a dry-run mode which goes through full rate-limiting logic
      to generate logs/metrics, but does not deny returns with 429
11. Okaaay, finally some work on rate limiting policies. So, the way I think
    about this is that there needs to be something that matches requests based
    on some criteria (e.g. who is making the request, which resource they're
    targeting), and then something else which says what the quota should be for
    such requests. So, I'm exploring a simple way of defining a policy: 
    - a list of matchers which determine whether a request should be subject to
      a policy
    - a quota which defines the limit and duration for the fixed-window algorith
      for requests matching the policy
    - a list of rate limit key parts which determine how a key is constructed
      for a request matching a policy. It's not necessary that the pieces used
      to match a request are the same exact pieces used to construct the key, so
      it makes sense to have this conceptual separation/distinction.
    - there is a method which takes relevant information for the requests, goes
      through the list of matchers to see which policies match the request, and
      then constructs the list of rate limit checks to be performed (= key +
      quota) for the request based on the matching policies.
    - again, for now, the policy is hardcoded, but eventually this could be
      loaded from a database or some configuration.
    - and for now, i'm implementing a small subset of matchers and key parts as
      a proof of concept.

Here's a mostly-sequence diagram which show the current flow:

```
                   ┌──────────────────┐        ┌──────────────────┐                                          
                   │                  │        │                  │                                          
                   │ Endpoint handler │        │  Rate limiter    │                                          
                   │                  │        │                  │                                          
                   └──────────────────┘        └──────────────────┘                                          
                            │                          │                                                     
                            │                          │                                                     
        Rate limit ────────▶│                          │                                                     
        policies            │                          │                                                     
                            │                          │   ┌────────────────────────────────────────────────┐
         Request   ────────▶│  Determine rate limit    │   │                                                │
                            │  checks for request      │   │ 1. Match requests to policies using matchers   │
                            │  based on policies       │   │    (e.g. based on endpoint or request method)  │
                            │─────────────────────────▶│   │ 2. For matched policies, construct the rate    │
                            │                          │──▶│    limit key based on the policy key template  │
                            │  List of rate limit      │   │ 3. Determine the quota for the fixed-window    │
                            │  checks                  │   │    rate limit algorithm                        │
                            │◀─────────────────────────│   └────────────────────────────────────────────────┘
                            │                          │   ┌────────────────────────────────────────────────┐
                            │                          │   │                                                │
                            │  Execute checks against  │   │ 1. Check keys' counters to determine if they   │
                            │  rate limit counters     │   │    are over the limit in the current window    │
                            │─────────────────────────▶│   │ 2. Return keys for which the limit was hit,    │
                            │                          │──▶│    the limits, and remaining window durations  │
                            │                          │   │ 3. Determine the max ramining window duration  │
                            │ Ok if all checks passed, │   │    for the retry-after header                  │
                            │ Err if some failed       │   └────────────────────────────────────────────────┘
                            │◀─────────────────────────│                                                     
429 response if             │                          │                                                     
rate limited, with ◀────────│                          │                                                     
Retry-After header          |
                            |
                            .
                            .
                      processing request
                      continues if not limited
```

12. I've been reading about how the metrics pipeline works so going to try to
    add some metrics for the rate limiting. The latency metrics already track
    the number of requests per endpoint and per response status, and since all
    limited requests result in a 429, that's good enough just for counting the
    number of limited requests. So, perhaps a metric which tracks limited
    requests per endpoint and per rate limit policy would be useful? Gonna try
    to add that. 
    - First, adding a new metric to http-service.toml since I think that makes
      sense as a target for now. 
    - Second, adding a producer which records rate limited requests as a simple
      counter (per policy and endpoint) and produces samples.
    - Third, adding a simple unit test to verify.
    - Fourth, integrate into the endpoint request flow so that metrics are
      actually produced, collected, and can be queried. Added an integration
      test for this to verify things are working -- wrote a simple oxql query to
      fetch metrics for an endpoint using the existing wait-until-metrics
      pattern for executing the query.
    - Aha! I had a bug -- I was looking at the raw query results and it turns
      out that even though the metric is stored as a cumulative sum, OxQL query
      results are constructed as deltas! Below is an example of query results.
      So, I need to take the sum of points, not the max since the sum of deltas
      represents the total number of limited requests.

```
OxqlTable {
    name: "http_service:rate_limited_request",
    timeseries: [
        Timeseries {
            fields: {
                "id": Uuid(
                    913233fe-92a8-4635-9572-183f495429c4,
                ),
                "name": String(
                    "nexus-external",
                ),
                "operation_id": String(
                    "current_user_view",
                ),
                "policy_id": String(
                    "current_user_view-policy",
                ),
            },
            points: Points {
                start_times: Some(
                    [
                        2026-05-27T11:33:10.794231Z,
                    ],
                ),
                timestamps: [
                    2026-05-27T11:33:10.819751Z,
                ],
                values: [
                    Values {
                        values: Integer(
                            [
                                Some(
                                    1,
                                ),
                            ],
                        ),
                        metric_type: Delta,
                    },
                ],
            },
            alignment: None,
        },
    ],
}
```

13. Short detour -- noticed a lot of cloning of checks in tests that felt
unnecessary. So tried to reduce that by changing the method to accept a
slice of borrowable checks instead of a slice of owned checks.

14. Okay, now something I wanted to do for a while -- add a new API endpoint.
So, I'm adding an endpoint for listing rate limit policies. This currently
returns the hardcoded policies, but in the future -- it would return policies
stored in the database. The API response looks like this:

    ```json
    GET /v1/system/rate-limit-policies
    
    [
      {
          "id":"current_user_view-policy",
          "matchers":[
            {
                "type":"endpoint",
                "any_of":[
                  "current_user_view"
                ]
            },
            {
                "type":"http_method",
                "any_of":[
                  "GET"
                ]
            }
          ],
          "quota":{
            "limit":2,
            "window_seconds":3600
          },
          "key_parts":[
            {
                "type":"literal",
                "value":"endpoint"
            },
            {
                "type":"http_method"
            },
            {
                "type":"endpoint"
            }
          ]
      },
      {
          "id":"user_builtin_list-policy",
          "matchers":[
            {
                "type":"endpoint",
                "any_of":[
                  "user_builtin_list"
                ]
            },
            {
                "type":"http_method",
                "any_of":[
                  "GET"
                ]
            }
          ],
          "quota":{
            "limit":2,
            "window_seconds":3600
          },
          "key_parts":[
            {
                "type":"literal",
                "value":"endpoint"
            },
            {
                "type":"http_method"
            },
            {
                "type":"endpoint"
            }
          ]
      },
      {
          "id":"global-policy",
          "matchers":[
            {
                "type":"global"
            }
          ],
          "quota":{
            "limit":2,
            "window_seconds":3600
          },
          "key_parts":[
            {
                "type":"literal",
                "value":"global"
            }
          ]
      }
    ]
    ```

    A few notes:
      - This makes me realize that the http_method matcher is not needed if
        there's an endpoint matcher on the policy since an endpoint is a
        combination of a path and a method already. So I've cleaned this up.
      - Since the policies are hardcoded, the test is "hardcoded" as wel -- it
        checks for specific policies to be returned.

15. I want to now try storing policies in the database. This is likely going to
    be a longer journey since I need to: define the DB schema and model, seed
    built-in policies into the DB, create a background task which syncs policies
    from the DB with nexus, update endpoint for listing policies to use DB, add
    an endpoint for creating policies, etc. 
      - First, I'm extracting the hardcoded policies into a separate file for
        built-in policies. Later, I'll move this to db-fixed-data so that it can
        be used to seed the policies in the DB.
      - Second, the DB tables and models. 
        - I'm adding a omicron.public.rate_limit_policy table which uses the
          standard identity metadata columns for API resources. Since rate limit
          policies will be something operators can list, create, delete, and
          update, it made sense to model them as resources. I'm also adding an
          "enabled" column for enabling/disabling a rate limit policy since this
          feels like something useful from the start.
        - I'm adding a omicron.public.rate_limit_policy_generation table similar
          to the tuf_generation table: there is a single row in this table and
          the generation is incremented when a policy is added, changed, or
          deleted. With this, a nexus process can check if their local
          generation number matches what is in the DB, and reload policies if
          they don't match.
        - Added the same changes as migrations as well.
        - Added diesel models which match the tables. No methods yet in this
          step.
        - Verified that `cargo nextest run -p omicron-nexus schema` passes.
      - Third, the DB query methods and seeding the DB with builtin policies.
        - I added only the bare minimum methods for getting the generation,
          fetching policies, and populating the policy table with builtin
          policies. I'll add more methods (e.g. for updating, deleting, etc)
          later.
        - I am NOT triggering the populating of the policy table on nexus
          startup yet, I'll do that in a followup commit. This commit only
          introduces the methods and tests them -- they are unused right now.
        - Also, the builtin policies in db-fixed-data mirror the policies from
          rate_limit_builtin.rs for now, but rate_limit_builtin.rs will be
          deleted later once I wire everything up.
      - Fourth, actually hooking up the populating of the rate_limit_policy
        table so that it happens on nexus startup.
        - Needed to tweak the query tests a bit since they expect an empty DB.
          Luckily, there is `raw_datastore_with_auth` for getting an empty DB
          and `OpContext::for_background` for creating `OpContext`s. Hooray!
      - Fifth, use the DB table as the source of truth for the API endpoint for
        listing rate limit policies. Some things I didn't do in this step and
        I will do next:
        - Checking/incrementing rate limits during request processing still uses
          the "old" in-memory rate limit policies.
        - Pagination is not implemented on the endpoint for listing policies,
          even though it's supported by the DB querying method.
        - The DB table doesn't fully match the old rate limit structure which is
          still returned by the API, e.g. enabled, UUID-based id, description
          exist in the DB records but not in the API response.
      - Next, added pagination to the endpoint and made the API response include
        the fields from the DB table. The API response looks like this now:
     
        ```json
        {
          "items": [
            {
              "description": "Built-in rate-limit policy for current-user view requests",
              "enabled": true,
              "id": "001de000-726c-4000-8000-000000000000",
              "key_parts": [
                {
                  "type": "literal",
                  "value": "endpoint"
                },
                {
                  "type": "endpoint"
                }
              ],
              "matchers": [
                {
                  "any_of": [
                    "current_user_view"
                  ],
                  "type": "endpoint"
                }
              ],
              "name": "current-user-view-policy",
              "quota": {
                "limit": 2,
                "window_seconds": 3600
              },
              "time_created": "2026-06-29T22:07:57.866497Z",
              "time_modified": "2026-06-29T22:07:57.866497Z"
            },
            {
              "description": "Built-in global rate-limit policy",
              "enabled": true,
              "id": "001de000-726c-4000-8000-000000000002",
              "key_parts": [
                {
                  "type": "literal",
                  "value": "global"
                }
              ],
              "matchers": [
                {
                  "type": "global"
                }
              ],
              "name": "global-policy",
              "quota": {
                "limit": 2,
                "window_seconds": 3600
              },
              "time_created": "2026-06-29T22:07:57.866501Z",
              "time_modified": "2026-06-29T22:07:57.866501Z"
            },
            {
              "description": "Built-in rate-limit policy for user-builtin list requests",
              "enabled": true,
              "id": "001de000-726c-4000-8000-000000000001",
              "key_parts": [
                {
                  "type": "literal",
                  "value": "endpoint"
                },
                {
                  "type": "endpoint"
                }
              ],
              "matchers": [
                {
                  "any_of": [
                    "user_builtin_list"
                  ],
                  "type": "endpoint"
                }
              ],
              "name": "user-builtin-list-policy",
              "quota": {
                "limit": 2,
                "window_seconds": 3600
              },
              "time_created": "2026-06-29T22:07:57.866500Z",
              "time_modified": "2026-06-29T22:07:57.866500Z"
            }
          ],
          "next_page": "eyJ2IjoidjEiLCJwYWdlX3N0YXJ0Ijp7InNvcnRfYnkiOiJuYW1lX2FzY2VuZGluZyIsImxhc3Rfc2VlbiI6InVzZXItYnVpbHRpbi1saXN0LXBvbGljeSJ9fQ=="
        }
        ```
      - Next, added a way to go from the db model types to internal types used
        for rate limiting.
          - I'm not too happy with the current solution in the sense that i feel
            this could be simpler. E.g. converting happens with the help of the
            API types since those already define how to deserialize json fields
            into types. So, perhaps move this deserialization into the db model
            type would reduce complexity. There's also some differences between
            other fiels which are likely unnecessary. E.g. the quota limit is a
            usize in the internal rate limit types, while an i64 in the db
            model.
          - Creating the internal rate limiting types from db data also needed
            a switch to support non-&'static str strings for types which use
            strings. Previously, internal rate limiting types were always
            loaded from hardcoded values, so using &'static str strings worked
            fine. So now I needed to switch to something that supports String.
            I switched everything to String for simplicity, but there are
            other options for the future, e.g. using Cow or a trait-bound
            dynamic type.
