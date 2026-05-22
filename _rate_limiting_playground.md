# What is this?

This is just a playground for me to explore adding rate limits to Nexus'
external API.

Rate limits are interesting to me in many ways (engineering, customer
experience, supportability) so this is a project/space for me to learn through
exploration in a production system I want get to know better.

The goal (a working rate limiting system) is not the only goal, the goal is
learning. So, I might do things in sub-optimal, roundabout ways for the purpose
of learning and exploration. 

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
