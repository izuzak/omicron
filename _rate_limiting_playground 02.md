(Continued from part 01)

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
      - Next, extended the RateLimitManager to hold the current set of rate
        limit policies.
          - The idea is: the DB is the source of truth for the policies and
            nexus instances have their local copy/snapshot of that truth (this
            is what the rate limit manager holds). There will be a background
            task which checks the source of truth to see if policies changed
            (this is why i added the generation several steps back), and if they
            did then the task would update the local copy so that it's again
            up-to-date.
          - Currently, this copy/snapshot is still seeded from the built-in
            hardcoded policies.
          - Since the same data (local policies) will be accessed from two paths
            (reading in request processing path when checking/enforcing rate
            limits and replacing in background task for refreshing policies), i
            added a rwlock for accessing the policies. I think it's okay for
            now, but there might be better ways of doing it, e.g. some Arc-based
            approach with cloning and swapping.
