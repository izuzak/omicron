CREATE TABLE IF NOT EXISTS omicron.public.rate_limit_policy (
    -- Identity metadata (resource)
    id UUID PRIMARY KEY,
    name STRING(63) NOT NULL,
    description STRING(512) NOT NULL,
    time_created TIMESTAMPTZ NOT NULL,
    time_modified TIMESTAMPTZ NOT NULL,
    time_deleted TIMESTAMPTZ,

    -- Policy configuration data
    enabled BOOL NOT NULL,
    quota_limit INT8 NOT NULL,
    quota_window_seconds INT8 NOT NULL,

    -- Matchers and key parts are stored as json for now for simplicity
    matchers JSONB NOT NULL,
    key_parts JSONB NOT NULL,

    -- Quota limit and window duration should be positive
    CONSTRAINT rate_limit_policy_quota_limit_positive
        CHECK (quota_limit > 0),
    CONSTRAINT rate_limit_policy_quota_window_positive
        CHECK (quota_window_seconds > 0)
);
