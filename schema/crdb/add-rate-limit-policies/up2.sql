INSERT INTO omicron.public.rate_limit_policy_generation (
    singleton,
    generation
) VALUES
    (TRUE, 1)
ON CONFLICT DO NOTHING;
