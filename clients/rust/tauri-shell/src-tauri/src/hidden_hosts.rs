use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn hidden_hosts_path() -> Result<PathBuf, String> {
    let pairing_store_path = maho_app::PairingStore::default_path()
        .map_err(|e| format!("Failed to get pairing store path: {e}"))?;
    let parent = pairing_store_path
        .parent()
        .ok_or_else(|| "Pairing store path has no parent directory".to_string())?;
    Ok(parent.join("hidden-hosts.json"))
}

pub fn load_hidden_hosts_from_path(path: &Path) -> Result<Vec<String>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read hidden hosts from {}: {e}", path.display()))?;
    if data.trim().is_empty() {
        return Ok(Vec::new());
    }
    let list: Vec<String> = serde_json::from_str(&data)
        .map_err(|e| format!("Failed to parse hidden hosts JSON: {e}"))?;

    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for name in list {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            let lower = trimmed.to_ascii_lowercase();
            if seen.insert(lower) {
                result.push(trimmed.to_string());
            }
        }
    }
    Ok(result)
}

pub fn save_hidden_hosts_to_path(path: &Path, list: &[String]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
    }

    let mut deduped = Vec::new();
    let mut seen = HashSet::new();
    for name in list {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            let lower = trimmed.to_ascii_lowercase();
            if seen.insert(lower) {
                deduped.push(trimmed.to_string());
            }
        }
    }

    let json = serde_json::to_string_pretty(&deduped)
        .map_err(|e| format!("Failed to serialize hidden hosts: {e}"))?;

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, json.as_bytes())
        .map_err(|e| format!("Failed to write temp hidden hosts file: {e}"))?;

    fs::rename(&tmp_path, path)
        .map_err(|e| format!("Failed to atomic rename hidden hosts file: {e}"))?;

    Ok(())
}

pub fn load_hidden_hosts() -> Result<Vec<String>, String> {
    let path = hidden_hosts_path()?;
    load_hidden_hosts_from_path(&path)
}

pub fn save_hidden_hosts(list: &[String]) -> Result<(), String> {
    let path = hidden_hosts_path()?;
    save_hidden_hosts_to_path(&path, list)
}

pub fn add_hidden_host(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let mut current = load_hidden_hosts()?;
    if !current
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(trimmed))
    {
        current.push(trimmed.to_string());
        save_hidden_hosts(&current)?;
    }
    Ok(())
}

pub fn remove_hidden_host(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let current = load_hidden_hosts()?;
    let filtered: Vec<String> = current
        .into_iter()
        .filter(|existing| !existing.eq_ignore_ascii_case(trimmed))
        .collect();
    save_hidden_hosts(&filtered)?;
    Ok(())
}

pub fn filter_hidden_hosts<T: AsRef<str>>(
    hosts: Vec<super::HostItem>,
    hidden_names: &[T],
) -> Vec<super::HostItem> {
    let hidden_set: HashSet<String> = hidden_names
        .iter()
        .map(|n| n.as_ref().trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    hosts
        .into_iter()
        .filter(|host| !hidden_set.contains(&host.name.trim().to_ascii_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_save_hidden_hosts_round_trip() {
        let temp_dir =
            std::env::temp_dir().join(format!("maho_test_hidden_{}", std::process::id()));
        let file_path = temp_dir.join("hidden-hosts.json");

        assert_eq!(
            load_hidden_hosts_from_path(&file_path).unwrap(),
            Vec::<String>::new()
        );

        let names = vec![
            "Codex-Environment".to_string(),
            "other-host".to_string(),
            "codex-environment".to_string(),
        ];
        save_hidden_hosts_to_path(&file_path, &names).unwrap();

        let loaded = load_hidden_hosts_from_path(&file_path).unwrap();
        assert_eq!(
            loaded,
            vec!["Codex-Environment".to_string(), "other-host".to_string()]
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_filter_hidden_hosts_pure_function() {
        let hosts = vec![
            super::super::HostItem {
                id: "1".into(),
                name: "codex-environment".into(),
                ip: "100.1.1.1".into(),
                os: "linux".into(),
                online: false,
                paired: false,
                last_seen: None,
                tcp_port: None,
                udp_port: None,
            },
            super::super::HostItem {
                id: "2".into(),
                name: "indo".into(),
                ip: "100.91.254.71".into(),
                os: "linux".into(),
                online: true,
                paired: true,
                last_seen: None,
                tcp_port: None,
                udp_port: None,
            },
        ];

        let filtered = filter_hidden_hosts(hosts, &["Codex-Environment"]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "indo");
    }
}
