use nix::unistd::{getuid, setgid, setgroups, setuid, Group, User};
use std::sync::atomic::{AtomicU32, Ordering};

pub const QUARK_USER_AND_GROUP: &str = "quark";

pub fn remove_last_slash(path: &str) -> &str {
    if path.ends_with("/") {
        &path[..path.len() - 1]
    } else {
        path
    }
}

pub fn format_ip(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some() => {
            v6.to_ipv4().unwrap().to_string()
        }
        _ => ip.to_string(),
    }
}

pub fn drop_privileges(name: &str) -> Result<&'static str, Box<dyn std::error::Error>> {
    // Check if we are already root.
    if !getuid().is_root() {
        return Ok("Privileges already dropped");
    }

    let user = User::from_name(name)?;
    let group = Group::from_name(name)?;

    if let (Some(user), Some(group)) = (user, group) {
        setgroups(&[group.gid])?;
        setgid(group.gid)?;
        setuid(user.uid)?;
    } else {
        return Err("User or group not found".into());
    }
    Ok("Privileges dropped")
}

pub fn extract_vars_from_string(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut pos = 0;
    while let Some(start) = text[pos..].find("${") {
        let start = pos + start;
        if let Some(end) = text[start..].find("}") {
            let end = start + end;
            let key = &text[start + 2..end];
            keys.push(key.to_string());
            pos = end + 1;
        } else {
            break;
        }
    }
    keys
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn generate_u32_id() -> u32 {
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed) as u32;
    counter
}
