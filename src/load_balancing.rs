use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};

use twox_hash::XxHash3_64;

use crate::config::Locations;

const ALGO_ROUND_ROBIN: &str = "round_robin";
const ALGO_IP_HASH: &str = "ip_hash";

#[derive(Debug)]
pub struct LoadBalancerConfig {
    round_robin: HashMap<u32, RoundRobinConfig>, // id -> RoundRobinConfig
}

#[derive(Debug)]
struct RoundRobinConfig {
    pub index: AtomicUsize,
    pub weights_indices: Option<Vec<usize>>,
}

impl LoadBalancerConfig {
    pub fn new(targets: Vec<&Locations>) -> Arc<Self> {
        let mut round_robin = HashMap::new();
        for target in targets {
            if let Some(algo) = &target.algo {
                // Create a config for round robin if defined.
                if ALGO_ROUND_ROBIN == algo.as_str() {
                    let mut rr_config = RoundRobinConfig {
                        index: AtomicUsize::new(0),
                        weights_indices: None,
                    };
                    // Configure weighted round robin if weights are set.
                    if let Some(weights) = &target.weights {
                        let mut weights_indices = vec![];
                        for (i, &weight) in weights.iter().enumerate() {
                            weights_indices.extend(std::iter::repeat(i).take(weight as usize));
                        }
                        rr_config.weights_indices = Some(weights_indices);
                    }
                    round_robin.insert(target.id, rr_config);
                }
            }
        }
        Arc::new(LoadBalancerConfig { round_robin })
    }

    pub fn balance(
        self: Arc<Self>,
        id: &u32,
        servers: &[String],
        algo: &Option<String>,
        ip: &str,
    ) -> String {
        let srv_nbr = servers.len();
        // Only one server or no loadbalancing config.
        if srv_nbr == 1 {
            return servers.first().unwrap().to_string();
        }
        if let Some(algo) = algo {
            match algo.as_str() {
                ALGO_ROUND_ROBIN => {
                    let rr = self.round_robin.get(id).unwrap();
                    let index = rr.index.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match &rr.weights_indices {
                        // Use weighted round robin.
                        Some(weights_indices) => {
                            return servers
                                .get(weights_indices[index % weights_indices.len()])
                                .unwrap()
                                .to_string();
                        }
                        // Use normal round robin.
                        None => {
                            return servers.get(index % srv_nbr).unwrap().to_string();
                        }
                    }
                }
                ALGO_IP_HASH => {
                    let hash = XxHash3_64::oneshot(ip.as_bytes());
                    let index = hash % srv_nbr as u64;
                    return servers.get(index as usize).unwrap().to_string();
                }
                _ => {}
            }
        }
        // Default.
        servers.first().unwrap().to_string()
    }
}

#[cfg(test)]
mod tests {
    use crate::config::TargetParams;

    use super::*;

    fn mock_load_balancer(weights: Option<Vec<u32>>, count: u8) -> Vec<String> {
        let location = Locations {
            id: 0,
            params: TargetParams {
                location: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                strict_uri: false,
                headers: None,
            },
            algo: Some("round_robin".to_string()),
            weights,
        };
        let lb = LoadBalancerConfig::new(vec![&location]);
        (0..count)
            .map(|_| {
                lb.clone().balance(
                    &location.id,
                    &location.params.location,
                    &location.algo,
                    "1.1.1.1",
                )
            })
            .collect()
    }

    #[test]
    fn test_round_robin() {
        let lb = mock_load_balancer(None, 4);
        assert_eq!(lb, vec!["a", "b", "c", "a"]);
    }

    #[test]
    fn test_weighted_round_robin() {
        let lb = mock_load_balancer(Some(vec![4, 2, 1]), 8);
        assert_eq!(lb, vec!["a", "a", "a", "a", "b", "b", "c", "a"]);
    }
}
