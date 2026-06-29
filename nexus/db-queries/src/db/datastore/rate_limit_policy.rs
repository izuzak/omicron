// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [`DataStore`] methods on rate-limit policies.

use super::DataStore;
use crate::authz;
use crate::context::OpContext;
use crate::db::model::Name;
use crate::db::model::RateLimitPolicy;
use crate::db::pagination::paginated;
use async_bb8_diesel::AsyncRunQueryDsl;
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use nexus_db_errors::ErrorHandler;
use nexus_db_errors::OptionalError;
use nexus_db_errors::public_error_from_diesel;
use nexus_db_lookup::DbConnection;
use omicron_common::api::external::Error;
use omicron_common::api::external::Generation;
use omicron_common::api::external::ListResultVec;
use omicron_common::api::external::LookupResult;
use omicron_common::api::external::http_pagination::PaginatedBy;
use ref_cast::RefCast;

// Helper enum that's used as a parameter to methods to drive filtering based
// on enabled/disabled policies
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLimitPolicyFilter {
    All,
    EnabledOnly,
}

impl DataStore {
    /// Returns the current rate limit policy generation number.
    pub async fn rate_limit_policy_generation_get(
        &self,
        opctx: &OpContext,
    ) -> LookupResult<Generation> {
        opctx.authorize(authz::Action::Read, &authz::FLEET).await?;

        get_generation(&*self.pool_connection_authorized(opctx).await?)
            .await
            .map_err(|e| public_error_from_diesel(e, ErrorHandler::Server))
    }

    /// Lists rate limit policies, with pagination
    pub async fn rate_limit_policy_list(
        &self,
        opctx: &OpContext,
        pagparams: &PaginatedBy<'_>,
        enabled_filter: RateLimitPolicyFilter,
    ) -> ListResultVec<RateLimitPolicy> {
        opctx.authorize(authz::Action::ListChildren, &authz::FLEET).await?;

        use nexus_db_schema::schema::rate_limit_policy::dsl;

        // Create a query with pagination
        let mut query = match pagparams {
            PaginatedBy::Id(params) => {
                paginated(dsl::rate_limit_policy, dsl::id, params)
            }
            PaginatedBy::Name(params) => paginated(
                dsl::rate_limit_policy,
                dsl::name,
                &params.map_name(|n| Name::ref_cast(n)),
            ),
        };

        // Add filter based on parameter
        match enabled_filter {
            RateLimitPolicyFilter::All => {}
            RateLimitPolicyFilter::EnabledOnly => {
                query = query.filter(dsl::enabled.eq(true));
            }
        }

        // Exclude deleted policies and run the query
        query
            .filter(dsl::time_deleted.is_null())
            .select(RateLimitPolicy::as_select())
            .load_async(&*self.pool_connection_authorized(opctx).await?)
            .await
            .map_err(|e| public_error_from_diesel(e, ErrorHandler::Server))
    }

    /// Stores builtin rate limit policies into the DB and bumps the generation
    pub async fn load_builtin_rate_limit_policies(
        &self,
        opctx: &OpContext,
    ) -> Result<(), Error> {
        use nexus_db_fixed_data::rate_limit_policy::BUILTIN_RATE_LIMIT_POLICIES;
        use nexus_db_schema::schema::rate_limit_policy::dsl;

        opctx.authorize(authz::Action::Modify, &authz::DATABASE).await?;

        let conn = self.pool_connection_authorized(opctx).await?;
        let err = OptionalError::new();

        // wrapping into a transaction since we want to store builting policies
        // and bump the generation as an atomic operation. Also, we insert the
        // policies only if policies with those same ids don't already exist.
        // If the policies exist but were changed in the meantime, they wont be
        // updated back to the initial state
        self.transaction_retry_wrapper("load_builtin_rate_limit_policies")
            .transaction(&conn, |txn| {
                let err = err.clone();
                async move {
                    // insert records
                    let inserted = diesel::insert_into(dsl::rate_limit_policy)
                        .values(&*BUILTIN_RATE_LIMIT_POLICIES)
                        .on_conflict(dsl::id)
                        .do_nothing()
                        .execute_async(&txn)
                        .await
                        .map_err(|e| {
                            err.bail_retryable_or_else(e, |e| {
                                public_error_from_diesel(
                                    e,
                                    ErrorHandler::Server,
                                )
                            })
                        })?;

                    if inserted > 0 {
                        // get current generation from db
                        let old_generation =
                            get_generation(&txn).await.map_err(|e| {
                                err.bail_retryable_or_else(e, |e| {
                                    public_error_from_diesel(
                                        e,
                                        ErrorHandler::Server,
                                    )
                                })
                            })?;

                        // write next generation to db
                        put_generation(
                            &txn,
                            old_generation.into(),
                            old_generation.next().into(),
                        )
                        .await
                        .map_err(|e| {
                            err.bail_retryable_or_else(e, |e| {
                                public_error_from_diesel(
                                    e,
                                    ErrorHandler::Server,
                                )
                            })
                        })?;
                    }

                    Ok(())
                }
            })
            .await
            .map_err(|e| match err.take() {
                Some(err) => err,
                None => public_error_from_diesel(e, ErrorHandler::Server),
            })
    }
}

// helper for getting the generation from the db via a connection parameter to
// make testing easier
async fn get_generation(
    conn: &async_bb8_diesel::Connection<DbConnection>,
) -> Result<Generation, DieselError> {
    use nexus_db_schema::schema::rate_limit_policy_generation::dsl;

    let generation: nexus_db_model::Generation =
        dsl::rate_limit_policy_generation
            .filter(dsl::singleton.eq(true))
            .select(dsl::generation)
            .get_result_async(conn)
            .await?;

    Ok(generation.0)
}

// helper for updating the generation in the db via a connection parameter to
// make testing easier
async fn put_generation(
    conn: &async_bb8_diesel::Connection<DbConnection>,
    old_generation: nexus_db_model::Generation,
    new_generation: nexus_db_model::Generation,
) -> Result<nexus_db_model::Generation, DieselError> {
    use nexus_db_schema::schema::rate_limit_policy_generation::dsl;

    // We use `get_result_async` instead of `execute_async` to check that we
    // updated exactly one row.
    diesel::update(dsl::rate_limit_policy_generation.filter(
        dsl::singleton.eq(true).and(dsl::generation.eq(old_generation)),
    ))
    .set(dsl::generation.eq(new_generation))
    .returning(dsl::generation)
    .get_result_async(conn)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authn;
    use crate::authz;
    use crate::db::pub_test_utils::TestDatabase;
    use chrono::DateTime;
    use chrono::Utc;
    use omicron_common::api::external::DataPageParams;
    use omicron_common::api::external::Name as ExternalName;
    use omicron_test_utils::dev;
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use uuid::Uuid;

    // test helper for creating rate limiting policies in the database
    async fn insert_rate_limit_policy(
        datastore: &DataStore,
        opctx: &OpContext,
        id: Uuid,
        name: &str,
        enabled: bool,
        time_deleted: Option<DateTime<Utc>>,
    ) {
        use nexus_db_schema::schema::rate_limit_policy::dsl;

        let now = Utc::now();
        diesel::insert_into(dsl::rate_limit_policy)
            .values((
                dsl::id.eq(id),
                dsl::name.eq(Name(name.parse().unwrap())),
                dsl::description.eq("test rate limit policy"),
                dsl::time_created.eq(now),
                dsl::time_modified.eq(now),
                dsl::time_deleted.eq(time_deleted),
                dsl::enabled.eq(enabled),
                dsl::quota_limit.eq(10_i64),
                dsl::quota_window_seconds.eq(60_i64),
                // empty matchers and key parts since we're not targeting those
                // in these tests
                dsl::matchers.eq(serde_json::json!([])),
                dsl::key_parts.eq(serde_json::json!([])),
            ))
            .execute_async(
                &*datastore.pool_connection_authorized(opctx).await.unwrap(),
            )
            .await
            .unwrap();
    }

    // helper to get policy names for a list of policies
    fn policy_names(policies: &[RateLimitPolicy]) -> Vec<String> {
        use nexus_types::identity::Resource;

        policies.iter().map(|policy| policy.name().to_string()).collect()
    }

    // helper to get sorted policy ids for a list of policies
    fn sorted_policy_ids(policies: &[RateLimitPolicy]) -> Vec<Uuid> {
        use nexus_types::identity::Resource;

        let mut ids =
            policies.iter().map(|policy| policy.id()).collect::<Vec<_>>();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn test_rate_limit_policy_generation_get() {
        let logctx =
            dev::test_setup_log("test_rate_limit_policy_generation_get");
        let db = TestDatabase::new_with_datastore(&logctx.log).await;
        let (opctx, datastore) = (db.opctx(), db.datastore());

        // Verify that the public method fetches the initial generation from the DB
        assert_eq!(
            datastore.rate_limit_policy_generation_get(opctx).await.unwrap(),
            Generation::new()
        );

        let next_generation = Generation::new().next();

        // Testing the internal helper and incrementing the DB generation
        {
            // Create a connection
            let conn =
                datastore.pool_connection_authorized(opctx).await.unwrap();

            // Verify that the private helper gets the initial generation from the DB
            assert_eq!(get_generation(&conn).await.unwrap(), Generation::new());

            // Verify that the internal helper increments the DB generation
            let updated_generation = put_generation(
                &conn,
                Generation::new().into(),
                next_generation.into(),
            )
            .await
            .unwrap();
            assert_eq!(
                updated_generation,
                nexus_db_model::Generation(next_generation)
            );

            // Verify that the internal helper sees the new generation
            assert_eq!(get_generation(&conn).await.unwrap(), next_generation);
        }

        // Verify that the public method sees the new generation
        assert_eq!(
            datastore.rate_limit_policy_generation_get(opctx).await.unwrap(),
            next_generation
        );

        db.terminate().await;
        logctx.cleanup_successful();
    }

    #[tokio::test]
    async fn test_rate_limit_policy_list() {
        let logctx = dev::test_setup_log("test_rate_limit_policy_list");
        let db = TestDatabase::new_with_datastore(&logctx.log).await;
        let (opctx, datastore) = (db.opctx(), db.datastore());

        // verify that the list of policies is empty
        let id_page = PaginatedBy::Id(DataPageParams {
            marker: None,
            direction: dropshot::PaginationOrder::Ascending,
            limit: NonZeroU32::new(10).unwrap(),
        });
        let policies = datastore
            .rate_limit_policy_list(opctx, &id_page, RateLimitPolicyFilter::All)
            .await
            .unwrap();
        assert!(policies.is_empty());

        // create an enabled, non-deleted policy
        insert_rate_limit_policy(
            datastore,
            opctx,
            "00000000-0000-0000-0000-000000000001".parse().unwrap(),
            "alpha",
            true,
            None,
        )
        .await;

        // create an disabled, non-deleted policy
        insert_rate_limit_policy(
            datastore,
            opctx,
            "00000000-0000-0000-0000-000000000002".parse().unwrap(),
            "beta",
            false,
            None,
        )
        .await;

        // create an enabled, deleted policy
        insert_rate_limit_policy(
            datastore,
            opctx,
            "00000000-0000-0000-0000-000000000003".parse().unwrap(),
            "gamma",
            true,
            Some(Utc::now()),
        )
        .await;

        // create another enabled, non-deleted policy
        insert_rate_limit_policy(
            datastore,
            opctx,
            "00000000-0000-0000-0000-000000000004".parse().unwrap(),
            "delta",
            true,
            None,
        )
        .await;

        // fetch first two policies again, ordered by name
        let first_name_page = PaginatedBy::Name(DataPageParams {
            marker: None,
            direction: dropshot::PaginationOrder::Ascending,
            limit: NonZeroU32::new(2).unwrap(),
        });
        let policies = datastore
            .rate_limit_policy_list(
                opctx,
                &first_name_page,
                RateLimitPolicyFilter::All,
            )
            .await
            .unwrap();
        assert_eq!(policy_names(&policies), ["alpha", "beta"]);

        // fetch two policies after "beta"
        let beta_marker: ExternalName = "beta".parse().unwrap();
        let second_name_page = PaginatedBy::Name(DataPageParams {
            marker: Some(&beta_marker),
            direction: dropshot::PaginationOrder::Ascending,
            limit: NonZeroU32::new(2).unwrap(),
        });
        let policies = datastore
            .rate_limit_policy_list(
                opctx,
                &second_name_page,
                RateLimitPolicyFilter::All,
            )
            .await
            .unwrap();
        assert_eq!(policy_names(&policies), ["delta"]);

        // fetch all enabled policies
        let policies = datastore
            .rate_limit_policy_list(
                opctx,
                &id_page,
                RateLimitPolicyFilter::EnabledOnly,
            )
            .await
            .unwrap();
        assert_eq!(policy_names(&policies), ["alpha", "delta"]);

        db.terminate().await;
        logctx.cleanup_successful();
    }

    // Builtin data loading happens during Nexus startup as the internal db-init
    // actor. So, we use the same background OpContext for testing
    // load_builtin_rate_limit_policies() because it authorizes Modify on
    // authz::DATABASE, which the normal test-privileged user is not allowed to
    // do.
    fn db_init_opctx(
        log: &slog::Logger,
        datastore: &Arc<DataStore>,
    ) -> OpContext {
        OpContext::for_background(
            log.clone(),
            Arc::new(authz::Authz::new(log)),
            authn::Context::internal_db_init(),
            Arc::clone(datastore) as Arc<dyn nexus_auth::storage::Storage>,
        )
    }

    #[tokio::test]
    async fn test_load_builtin_rate_limit_policies() {
        use nexus_db_fixed_data::rate_limit_policy::BUILTIN_RATE_LIMIT_POLICIES;

        let logctx =
            dev::test_setup_log("test_load_builtin_rate_limit_policies");
        let db = TestDatabase::new_with_datastore(&logctx.log).await;
        let (opctx, datastore) = (db.opctx(), db.datastore());
        let db_init_opctx = db_init_opctx(&logctx.log, datastore);

        // check that we're at the initial generation
        assert_eq!(
            datastore.rate_limit_policy_generation_get(opctx).await.unwrap(),
            Generation::new()
        );

        // check that the table is empty i.e. there are no policies yet
        let id_page = PaginatedBy::Id(DataPageParams {
            marker: None,
            direction: dropshot::PaginationOrder::Ascending,
            limit: NonZeroU32::new(10).unwrap(),
        });
        assert!(
            datastore
                .rate_limit_policy_list(
                    opctx,
                    &id_page,
                    RateLimitPolicyFilter::All,
                )
                .await
                .unwrap()
                .is_empty()
        );

        // load the builtin policies
        datastore
            .load_builtin_rate_limit_policies(&db_init_opctx)
            .await
            .unwrap();

        // check that the DB is on the next generation
        let expected_generation = Generation::new().next();
        assert_eq!(
            datastore.rate_limit_policy_generation_get(opctx).await.unwrap(),
            expected_generation
        );

        // check that the database has the builtin policies now
        let policies = datastore
            .rate_limit_policy_list(opctx, &id_page, RateLimitPolicyFilter::All)
            .await
            .unwrap();
        assert_eq!(
            sorted_policy_ids(&policies),
            sorted_policy_ids(&BUILTIN_RATE_LIMIT_POLICIES)
        );

        // try loading the builtin policies again
        datastore
            .load_builtin_rate_limit_policies(&db_init_opctx)
            .await
            .unwrap();

        // verify that the generation didn't change since nothing new was inserted
        assert_eq!(
            datastore.rate_limit_policy_generation_get(opctx).await.unwrap(),
            expected_generation
        );

        // check that there are no new policies
        let policies_after_second_load = datastore
            .rate_limit_policy_list(opctx, &id_page, RateLimitPolicyFilter::All)
            .await
            .unwrap();
        assert_eq!(
            sorted_policy_ids(&policies_after_second_load),
            sorted_policy_ids(&policies)
        );

        db.terminate().await;
        logctx.cleanup_successful();
    }
}
