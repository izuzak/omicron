# What is this?

This is just a playground for me to explore adding rate limits to Nexus'
external API.

Rate limits are interesting to me in many ways (engineering, customer
experience, supportability) so this is a project/space for me to learn through
exploration in a production system I want get to know better.

The goal (a working rate limiting system) is not the only goal, the goal is learning. So, I might do things in sub-optimal, roundabout ways for the purpose of learning and exploration. 

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

# Nexus APIs

Here's what I want to try:
1. Create or duplicate a test which makes a single request to API endpoint A and endpoint B. Pick some endpoints which don't change state. This should pass.
2. Change the test to make 3 requests to endpoint A and 3 request to endpoint B. This should still pass.
3. Implement the simplest possible hardcoded counter which rejects requests to endpoint A after 2 requests. Third request to endpoint A should be rejected with 429, while all requests to endpoint B should pass.
