use nix::unistd::{getuid, setgid, setgroups, setuid, Group, User};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

pub const QUARK_USER_AND_GROUP: &str = "quark";

pub fn remove_last_slash(path: &str) -> &str {
    if let Some(p) = path.strip_suffix("/") {
        p
    } else {
        path
    }
}

pub fn get_path_and_file(path_str: &str) -> (PathBuf, Option<PathBuf>) {
    let path = Path::new(path_str);

    if path.is_file() {
        let parent = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let file = path.file_name().map(PathBuf::from);
        (parent, file)
    } else {
        (path.to_path_buf(), None)
    }
}

pub fn get_base_path(path: &str) -> &str {
    match path.find(['?', '#']) {
        Some(index) => &path[..index],
        None => path,
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

pub fn get_project_version() -> String {
    let version = format!("{} v.{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    #[cfg(debug_assertions)]
    {
        let git_hash = env!("GIT_HASH");
        format!("{version}-dev+{git_hash}")
    }
    #[cfg(not(debug_assertions))]
    version
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn generate_u32_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

const KB: f64 = 1024.0;
const MB: f64 = KB * 1024.0;
const GB: f64 = MB * 1024.0;
const TB: f64 = GB * 1024.0;

pub fn format_size(size: u64) -> String {
    let size_f = size as f64;

    if size_f < KB {
        format!("{size} B")
    } else if size_f < MB {
        format!("{:.1} KB", size_f / KB)
    } else if size_f < GB {
        format!("{:.1} MB", size_f / MB)
    } else if size_f < TB {
        format!("{:.1} GB", size_f / GB)
    } else {
        format!("{:.1} TB", size_f / TB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_single_var() {
        let text = "My ${variable}";
        let var = extract_vars_from_string(text);
        assert_eq!(var, ["variable"]);
    }
    #[test]
    fn extract_multiples_var() {
        let text = "${var1} and ${var2} and ${var3}";
        let var = extract_vars_from_string(text);
        assert_eq!(var, ["var1", "var2", "var3"]);
    }
}
