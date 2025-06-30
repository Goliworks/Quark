use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc},
};

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
    ) -> String {
        let srv_nbr = servers.len();
        if srv_nbr == 1 {
            return servers.get(0).unwrap().to_string();
        }
        if let Some(algo) = algo {
            match algo.as_str() {
                ALGO_ROUND_ROBIN => {
                    // To do : round robin.
                }
                ALGO_IP_HASH => {
                    // To do : ip hash.
                }
                _ => {}
            }
        }
        servers.get(0).unwrap().to_string()
    }
}
