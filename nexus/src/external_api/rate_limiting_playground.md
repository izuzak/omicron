# What is this?

This is just a playground for me to explore adding rate limits to Nexus'
external API.

Rate limits are interesting to me in many ways (engineering, customer
experience, supportability) so this is a project/space for me to learn through
exploration in a production system I want get to know better.

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
