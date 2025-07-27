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
pub struct RoundRobinConfig {
    pub index: AtomicUsize,
    pub weights_indices: Option<Vec<usize>>,
}

impl LoadBalancerConfig {
    pub fn new(targets: Vec<&Locations>) -> Arc<Self> {
        let mut round_robin = HashMap::new();
        for target in targets {
            if let Some(algo) = &target.algo {
                match algo.as_str() {
                    ALGO_ROUND_ROBIN => {
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
                    _ => {}
                }
            }
        }
        Arc::new(LoadBalancerConfig { round_robin })
    }

    pub fn balance(
        self: Arc<Self>,
        id: &u32,
        servers: &Vec<String>,
        algo: &Option<String>,
        ip: &str,
    ) -> String {
        let srv_nbr = servers.len();
        // Only one server or no loadbalancing config.
        if srv_nbr == 1 {
            return servers.get(0).unwrap().to_string();
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
        servers.get(0).unwrap().to_string()
    }
}
