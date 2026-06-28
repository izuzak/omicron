CREATE UNIQUE INDEX IF NOT EXISTS lookup_rate_limit_policy_by_name
ON omicron.public.rate_limit_policy (
    name
)
WHERE
    time_deleted IS NULL;
