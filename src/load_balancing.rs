use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};

use twox_hash::XxHash3_64;

use crate::config::Target;

const ALGO_ROUND_ROBIN: &str = "round_robin";
const ALGO_IP_HASH: &str = "ip_hash";

#[derive(Debug)]
pub struct LoadBalancerConfig {
    round_robin: HashMap<u32, AtomicUsize>, // id -> index
}

impl LoadBalancerConfig {
    pub fn new(targets: Vec<&Target>) -> Self {
        let mut round_robin = HashMap::new();
        for target in targets {
            if let Some(algo) = &target.algo {
                match algo.as_str() {
                    ALGO_ROUND_ROBIN => {
                        round_robin.insert(target.id, AtomicUsize::new(0));
                    }
                    _ => {}
                }
            }
        }
        LoadBalancerConfig { round_robin }
    }

    pub fn balance(
        self: Arc<Self>,
        id: &u32,
        servers: &Vec<String>,
        algo: &Option<String>,
        ip: &str,
    ) -> String {
        let srv_nbr = servers.len();
        if srv_nbr == 1 {
            return servers.get(0).unwrap().to_string();
        }
        if let Some(algo) = algo {
            match algo.as_str() {
                ALGO_ROUND_ROBIN => {
                    let index = self
                        .round_robin
                        .get(id)
                        .unwrap()
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return servers.get(index % srv_nbr).unwrap().to_string();
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
