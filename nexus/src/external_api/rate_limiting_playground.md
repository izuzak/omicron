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
