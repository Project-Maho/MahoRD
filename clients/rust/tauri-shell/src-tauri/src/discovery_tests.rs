use super::commands::hosts_from_tailscale_output;

#[cfg(unix)]
fn output(code: i32, stdout: &[u8]) -> std::io::Result<std::process::Output> {
    use std::os::unix::process::ExitStatusExt;
    Ok(std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    })
}

#[test]
fn empty_observation_never_invents_online_testbeds() {
    let hosts = hosts_from_tailscale_output(output(0, br#"{"Peer":{}}"#), &[]).unwrap();
    assert!(hosts.is_empty(), "empty observation produced {hosts:?}");
}

#[cfg(windows)]
fn output(code: i32, stdout: &[u8]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::ExitStatusExt;
    Ok(std::process::Output {
        status: std::process::ExitStatus::from_raw(code as u32),
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    })
}

#[test]
fn missing_command_is_an_error_not_hosts() {
    let result =
        hosts_from_tailscale_output(Err(std::io::Error::from(std::io::ErrorKind::NotFound)), &[]);
    assert!(result.is_err());
}

#[test]
fn nonzero_exit_rejects_even_valid_peer_json() {
    assert!(hosts_from_tailscale_output(output(1, br#"{"Peer":{}}"#), &[]).is_err());
}

#[test]
fn malformed_status_is_an_error_not_an_empty_success() {
    for json in [
        "not JSON",
        "{}",
        "null",
        r#"{"Peer":[]}"#,
        r#"{"Peer":{"p":{}}}"#,
        r#"{"Peer":{"p":{"HostName":"host","Online":"true","TailscaleIPs":["100.64.0.9"]}}}"#,
        r#"{"Peer":{"p":{"HostName":"host","Online":true,"TailscaleIPs":"100.64.0.9"}}}"#,
    ] {
        assert!(hosts_from_tailscale_output(output(0, json.as_bytes()), &[]).is_err());
    }
}

#[test]
fn null_peer_map_has_no_hosts() {
    assert!(
        hosts_from_tailscale_output(output(0, br#"{"Peer":null}"#), &[])
            .unwrap()
            .is_empty()
    );
}

fn record(id: &str, name: &str) -> maho_app::PairingRecord {
    maho_app::PairingRecord {
        id: id.into(),
        name: name.into(),
        key: vec![0; 32],
        added_at_unix_ms: 0,
        last_endpoint: None,
        endpoint_aliases: Vec::new(),
    }
}

#[test]
fn observed_known_and_other_peers_keep_presence_and_json_fields() {
    let status = serde_json::json!({"Peer": {
        "a": {"HostName":"indo", "OS":"linux", "Online":false,
              "TailscaleIPs":["100.91.254.71", "fd7a:115c:a1e0::1"]},
        "b": {"HostName":"DESKTOP-1LAPJMP", "OS":"windows", "Online":true,
              "TailscaleIPs":["100.126.171.58"]},
        "c": {"HostName":"other-host", "OS":"linux", "Online":true,
              "TailscaleIPs":["100.64.0.9"]}
    }});
    let records = [record("old", "indo"), record("latest", "indo")];
    let hosts =
        hosts_from_tailscale_output(output(0, &serde_json::to_vec(&status).unwrap()), &records)
            .unwrap();
    assert_eq!(
        serde_json::to_value(hosts).unwrap(),
        serde_json::json!([
            {"id":"latest", "name":"indo", "ip":"100.91.254.71", "os":"linux",
             "online":false, "paired":true, "last_seen":null, "tcp_port":null, "udp_port":null},
            {"id":"100.126.171.58", "name":"DESKTOP-1LAPJMP", "ip":"100.126.171.58", "os":"windows",
             "online":true, "paired":false, "last_seen":null, "tcp_port":null, "udp_port":null},
            {"id":"100.64.0.9", "name":"other-host", "ip":"100.64.0.9", "os":"linux",
             "online":true, "paired":false, "last_seen":null, "tcp_port":null, "udp_port":null}
        ])
    );
}

#[test]
fn pairing_does_not_infer_identity_from_testbed_ips_case_or_substrings() {
    let status = serde_json::json!({"Peer": {
        "a": {"HostName":"different-linux", "Online":true, "TailscaleIPs":["100.91.254.71"]},
        "b": {"HostName":"different-windows", "Online":true, "TailscaleIPs":["100.126.171.58"]},
        "c": {"HostName":"INDO", "Online":true, "TailscaleIPs":["100.64.0.9"]},
        "d": {"HostName":"DESKTOP", "Online":true, "TailscaleIPs":["100.64.0.10"]}
    }});
    let records = [
        record("linux-key", "indo"),
        record("windows-key", "DESKTOP-old"),
    ];
    let hosts =
        hosts_from_tailscale_output(output(0, &serde_json::to_vec(&status).unwrap()), &records)
            .unwrap();
    assert_eq!(hosts.len(), 4);
    for host in hosts {
        assert!(!host.paired);
        assert_eq!(host.id, host.ip);
    }
}

#[test]
fn stored_pairings_and_self_do_not_create_observed_peers() {
    let records = [record("old", "indo"), record("other", "DESKTOP-1LAPJMP")];
    let status =
        br#"{"Peer":{},"Self":{"HostName":"indo","Online":true,"TailscaleIPs":["100.91.254.71"]}}"#;
    assert!(hosts_from_tailscale_output(output(0, status), &records)
        .unwrap()
        .is_empty());
}

#[test]
fn non_host_mobile_platforms_are_excluded_from_host_list() {
    let status = serde_json::json!({"Peer": {
        "iphone": {"HostName":"localhost", "OS":"iOS", "Online":true,
                   "TailscaleIPs":["100.114.81.32"]},
        "android": {"HostName":"pixel8", "OS":"Android", "Online":true,
                    "TailscaleIPs":["100.64.0.15"]},
        "desktop": {"HostName":"indo", "OS":"linux", "Online":true,
                    "TailscaleIPs":["100.91.254.71"]}
    }});
    let hosts =
        hosts_from_tailscale_output(output(0, &serde_json::to_vec(&status).unwrap()), &[]).unwrap();
    assert_eq!(hosts.len(), 1, "only desktop host platforms must be listed");
    assert_eq!(hosts[0].name, "indo");
    assert_eq!(hosts[0].ip, "100.91.254.71");
}

fn lan_host(
    id: &str,
    name: &str,
    ip: &str,
    os: &str,
    tcp_port: u16,
    udp_port: u16,
) -> maho_net::discovery::DiscoveredHost {
    maho_net::discovery::DiscoveredHost {
        id: id.into(),
        name: name.into(),
        ip: ip.into(),
        os: os.into(),
        tcp_port,
        udp_port,
    }
}

#[test]
fn lan_survives_missing_or_failed_tailscale() {
    let lan = Ok(vec![lan_host(
        "host1._maho-rd._tcp.local.",
        "host1",
        "192.168.1.50",
        "linux",
        19730,
        19731,
    )]);
    let tailscale = Err("Tailscale status execution failed: not found".to_string());
    let merged = super::commands::merge_discovery_results(lan, tailscale, &[]).unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "host1._maho-rd._tcp.local.");
    assert_eq!(merged[0].ip, "192.168.1.50");
    assert!(merged[0].online);
    assert_eq!(merged[0].tcp_port, Some(19730));
    assert_eq!(merged[0].udp_port, Some(19731));
}

#[test]
fn source_dedup_prioritizes_lan_over_tailscale() {
    let lan = Ok(vec![lan_host(
        "desk._maho-rd._tcp.local.",
        "desk-lan",
        "100.91.254.71",
        "linux",
        19740,
        19741,
    )]);
    let tailscale = Ok(vec![super::HostItem {
        id: "100.91.254.71".into(),
        name: "desk-ts".into(),
        ip: "100.91.254.71".into(),
        os: "linux".into(),
        online: false,
        paired: false,
        last_seen: None,
        tcp_port: None,
        udp_port: None,
    }]);
    let merged = super::commands::merge_discovery_results(lan, tailscale, &[]).unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "desk._maho-rd._tcp.local.");
    assert_eq!(merged[0].name, "desk-lan");
    assert!(merged[0].online);
    assert_eq!(merged[0].tcp_port, Some(19740));
    assert_eq!(merged[0].udp_port, Some(19741));
}

#[test]
fn source_dedup_keeps_identical_name_distinct_endpoints_and_does_not_inherit_pairing() {
    let lan = Ok(vec![lan_host(
        "desktop-lan._maho-rd._tcp.local.",
        "DESKTOP-1LAPJMP",
        "192.168.0.60",
        "windows",
        19730,
        19731,
    )]);
    let tailscale = Ok(vec![super::HostItem {
        id: "paired-uuid-1234".into(),
        name: "DESKTOP-1LAPJMP".into(),
        ip: "100.126.171.58".into(),
        os: "windows".into(),
        online: true,
        paired: true,
        last_seen: None,
        tcp_port: None,
        udp_port: None,
    }]);
    let records = [record("paired-uuid-1234", "DESKTOP-1LAPJMP")];
    let merged = super::commands::merge_discovery_results(lan, tailscale, &records).unwrap();
    // Discovery remains untrusted; keep identical-name distinct endpoints and do not inherit trust through dedup
    assert_eq!(
        merged.len(),
        2,
        "identical-name distinct endpoints must NOT be deduplicated into one card"
    );
    let lan_card = merged
        .iter()
        .find(|h| h.ip == "192.168.0.60")
        .expect("lan card present");
    assert_eq!(lan_card.name, "DESKTOP-1LAPJMP");
    assert!(
        !lan_card.paired,
        "LAN card must remain unpaired; must not inherit pairing trust from Tailscale"
    );
    assert_eq!(lan_card.id, "desktop-lan._maho-rd._tcp.local.");
    assert_eq!(lan_card.tcp_port, Some(19730));
    assert_eq!(lan_card.udp_port, Some(19731));

    let ts_card = merged
        .iter()
        .find(|h| h.ip == "100.126.171.58")
        .expect("ts card present");
    assert_eq!(ts_card.name, "DESKTOP-1LAPJMP");
    assert!(ts_card.paired);
    assert_eq!(ts_card.id, "paired-uuid-1234");
}

#[test]
fn empty_successful_lan_with_failed_tailscale_yields_empty_success() {
    let lan = Ok(vec![]);
    let tailscale = Err("Tailscale status execution failed: not found".to_string());
    let merged = super::commands::merge_discovery_results(lan, tailscale, &[]).unwrap();
    assert!(merged.is_empty());
}

#[test]
fn both_sources_failing_yields_an_error() {
    let lan = Err(maho_net::discovery::DiscoveryError::Backend(
        "LAN daemon unavailable".into(),
    ));
    let tailscale = Err("Tailscale status execution failed: not found".to_string());
    let result = super::commands::merge_discovery_results(lan, tailscale, &[]);
    assert!(result.is_err());
}

#[test]
fn lan_host_with_matching_stored_name_remains_unpaired() {
    let records = [record("auth-pair-key-123", "indo")];
    let lan = Ok(vec![lan_host(
        "indo._maho-rd._tcp.local.",
        "indo",
        "192.168.1.150",
        "linux",
        19730,
        19731,
    )]);
    let tailscale = Ok(vec![]);
    let merged = super::commands::merge_discovery_results(lan, tailscale, &records).unwrap();
    assert_eq!(merged.len(), 1);
    assert!(!merged[0].paired);
    assert_eq!(merged[0].id, "indo._maho-rd._tcp.local.");
    assert_ne!(merged[0].id, "auth-pair-key-123");
}

#[tokio::test]
async fn absent_executable_uses_the_actual_async_command_seam() {
    let absent = std::env::current_exe()
        .unwrap()
        .with_extension("discovery-absent");
    assert!(!absent.exists());
    let output = super::commands::tailscale_status(absent.as_os_str()).await;
    assert_eq!(
        output.as_ref().unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(hosts_from_tailscale_output(output, &[]).is_err());
}

#[tokio::test]
#[ignore = "explicit installed-Tailscale observation; run with --ignored --nocapture"]
async fn observe_installed_tailscale() -> Result<(), String> {
    // Calls the Tauri command itself; no fixture, PATH mutation, or store copying.
    let hosts = super::commands::list_hosts_default().await?;
    let observed: Vec<_> = hosts
        .iter()
        .map(|host| {
            serde_json::json!({
                "name":host.name, "ip":host.ip, "os":host.os,
                "online":host.online, "paired":host.paired
            })
        })
        .collect();
    println!(
        "ACTUAL_DISCOVERY {}",
        serde_json::to_string(&observed).unwrap()
    );
    Ok(())
}

#[tokio::test]
async fn managed_state_lan_retry_does_not_permanently_lock_initialization_error() {
    let state = super::AppState::default();
    assert!(state.discovery.lan_browser.lock().unwrap().is_none());
    let _ = super::commands::list_hosts_internal(&state).await;
    // The slot may hold a browser or stay empty when LAN discovery cannot
    // initialize here; what must not happen is the lock staying poisoned or
    // held, which would make every later retry fail.
    assert!(
        state.discovery.lan_browser.try_lock().is_ok(),
        "lan_browser lock was left held after list_hosts_internal"
    );
}

#[tokio::test]
async fn list_hosts_internal_returns_lan_immediately_without_waiting_for_slow_tailscale() {
    let state = super::AppState::default();
    let start = std::time::Instant::now();
    let _ = super::commands::list_hosts_internal(&state).await;
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "list_hosts must not wait 5s for Tailscale"
    );
}
