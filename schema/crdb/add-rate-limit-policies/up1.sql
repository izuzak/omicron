CREATE TABLE IF NOT EXISTS omicron.public.rate_limit_policy_generation (
    -- There should only be one row of this table for the whole DB.
    -- It's a little goofy, but filter on "singleton = true" before querying
    -- or applying updates, and you'll access the singleton row.
    --
    -- We also add a constraint on this table to ensure it's not possible to
    -- access the version of this table with "singleton = false".
    singleton BOOL NOT NULL PRIMARY KEY,
    -- Generation number owned and incremented by Nexus
    generation INT8 NOT NULL,

    CHECK (singleton = true),
    CHECK (generation > 0)
);
