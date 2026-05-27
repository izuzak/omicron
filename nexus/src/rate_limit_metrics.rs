// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use oximeter::types::Cumulative;
use oximeter::{MetricsError, Producer, Sample};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

oximeter::use_timeseries!("http-service.toml");

use http_service::{HttpService, RateLimitedRequest};

// Producer for Nexus' rate limiter metrics. For now, I think the target can be
// the HttpService since it's the service that's limiting the requests. But we
// could also use a more precise target, e.g. a rate limiter. Also, currently
// this tracks the number of limited requests per policy and operation. This
// could be expanded in various ways in the future, e.g. if we want to have a
// dry-run mode for the limiter we'd add a new field to the metric as well.
#[derive(Debug, Clone)]
pub(crate) struct RateLimitMetrics {
    target: HttpService,
    limited_requests: Arc<Mutex<HashMap<MetricKeyFields, RateLimitedRequest>>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct MetricKeyFields {
    policy_id: String,
    operation_id: String,
}

impl RateLimitMetrics {
    pub(crate) fn new(nexus_id: Uuid, service_name: &'static str) -> Self {
        Self {
            target: HttpService { id: nexus_id, name: service_name.into() },
            limited_requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // Called by Nexus' APIs to count rate limited requests. A single
    // request could be limited by multiple policies, so record_limited_request
    // could be called multiple times for a single request. As a result, the
    // sum of counters is going to be higher than the total number of requests.
    pub(crate) fn record_limited_request(
        &self,
        policy_id: &str,
        operation_id: &str,
    ) {
        let key = MetricKeyFields {
            policy_id: policy_id.to_string(),
            operation_id: operation_id.to_string(),
        };

        let mut limited_requests = self.limited_requests.lock().unwrap();
        limited_requests
            .entry(key.clone())
            .or_insert_with(|| RateLimitedRequest {
                policy_id: key.policy_id.into(),
                operation_id: key.operation_id.into(),
                datum: Cumulative::default(),
            })
            .datum += 1;
    }
}

impl Producer for RateLimitMetrics {
    fn produce(
        &mut self,
    ) -> Result<Box<dyn Iterator<Item = Sample> + 'static>, MetricsError> {
        let limited_requests = self
            .limited_requests
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();

        let target = self.target.clone();
        let samples = limited_requests
            .into_iter()
            .map(|decision| Sample::new(&target, &decision))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Box::new(samples.into_iter()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limited_requests_are_recorded_in_samples() {
        let nexus_id = Uuid::new_v4();
        let service_name = "test_service";
        let mut producer = RateLimitMetrics::new(nexus_id, service_name);

        // Verify that there are no samples at the start
        assert_eq!(producer.produce().unwrap().count(), 0);

        let policy_id = "some_policy";
        let operation_id = "some_endpoint";

        // Record a limited request
        producer.record_limited_request(policy_id, operation_id);

        // Verify that there is a single sample now
        assert_eq!(producer.produce().unwrap().count(), 1);
    }
}
