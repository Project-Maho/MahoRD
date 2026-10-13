use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionsDto {
    keyboard: bool,
    mouse: bool,
    gamepad: bool,
    clipboard: bool,
    audio: bool,
}

impl From<maho_proto::GuestPermissions> for PermissionsDto {
    fn from(p: maho_proto::GuestPermissions) -> Self {
        Self {
            keyboard: p.keyboard,
            mouse: p.mouse,
            gamepad: p.gamepad,
            clipboard: p.clipboard,
            audio: p.audio,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoDto {
    pub guest_names: Option<Vec<String>>,
    pub permissions: Option<PermissionsDto>,
    pub ended: Option<String>,
    pub trust: Option<String>,
    pub reconnect: String,
    pub attempt: Option<u32>,
    pub next_retry_ms: Option<u64>,
    pub reason: Option<String>,
    pub sas_request_id: Option<u32>,
    pub sas_result: Option<String>,
}

pub fn session_info(session: &ClientSession) -> Result<SessionInfoDto, String> {
    let info = session.session_info().map_err(|e| e.to_string())?;
    let (reconnect, attempt, next_retry_ms, reason) = match info.reconnect {
        maho_app::ReconnectState::Idle => ("idle", None, None, None),
        maho_app::ReconnectState::Reconnected => ("reconnected", None, None, None),
        maho_app::ReconnectState::Reconnecting {
            attempt,
            next_retry_ms,
        } => ("reconnecting", Some(attempt), Some(next_retry_ms), None),
        maho_app::ReconnectState::GaveUp { reason } => ("gave-up", None, None, Some(reason)),
    };
    Ok(SessionInfoDto {
        guest_names: info.host_status.map(|s| s.guest_names),
        permissions: info.permissions.map(Into::into),
        ended: info.ended.map(|r| format!("{r:?}")),
        trust: info.pairing_trust.map(|r| format!("{r:?}")),
        reconnect: reconnect.into(),
        attempt,
        next_retry_ms,
        reason,
        sas_request_id: info.secure_attention_response.map(|r| r.request_id),
        sas_result: info
            .secure_attention_response
            .map(|r| format!("{:?}", r.result)),
    })
}

fn session(state: &AppState) -> Result<ClientSession, String> {
    state
        .session
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "No active session".into())
}

#[tauri::command]
pub fn cancel_connect(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(session) = state.session.lock().map_err(|e| e.to_string())?.as_ref() {
        session.interrupt_handle().interrupt();
    }
    Ok(())
}

#[tauri::command]
pub fn set_client_clipboard(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let session = session(&state)?;
    session
        .set_clipboard_enabled(enabled)
        .map_err(|e| e.to_string())?;
    state.clipboard_enabled.store(enabled, Ordering::SeqCst);
    commands::stop_clipboard_monitor(&state);
    if enabled {
        commands::start_clipboard_monitor(&state, &session);
    }
    Ok(())
}

#[tauri::command]
pub fn request_secure_attention(state: State<'_, AppState>) -> Result<u32, String> {
    session(&state)?
        .request_secure_attention()
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct GuestDto {
    permissions: Option<PermissionsDto>,
    id: String,
    pairing_id: String,
    name: String,
    trust: String,
    connected_seconds: u64,
}

#[derive(Serialize)]
pub struct PendingDto {
    id: String,
    name: String,
    expires_in_ms: u128,
}

#[derive(Serialize)]
pub struct GuestsDto {
    sessions: Vec<GuestDto>,
    pending: Vec<PendingDto>,
    events: Vec<String>,
    error: Option<String>,
}

fn registry(state: &AppState) -> Result<Arc<maho_host::guests::GuestRegistry>, String> {
    state
        .host_runtime
        .guests
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "Hosting is not running".into())
}

#[tauri::command]
pub fn host_guests(
    state: State<'_, AppState>,
    drain_events: Option<bool>,
) -> Result<GuestsDto, String> {
    let registry = state
        .host_runtime
        .guests
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let mut events = Vec::new();
    if let Some(rx) = state
        .host_runtime
        .events
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .filter(|_| drain_events.unwrap_or(true))
    {
        while let Ok(event) = rx.try_recv() {
            match event {
                maho_host::guests::HostEvent::GuestConnected(g) => {
                    events.push(format!("{} connected", g.name))
                }
                maho_host::guests::HostEvent::GuestDisconnected(g) => {
                    events.push(format!("{} disconnected", g.name))
                }
                maho_host::guests::HostEvent::ConsentRequested(_) => {}
            }
        }
    }
    Ok(GuestsDto {
        sessions: registry
            .as_ref()
            .map(|r| {
                r.sessions()
                    .into_iter()
                    .map(|g| GuestDto {
                        permissions: maho_host::PairingStore::host_default()
                            .and_then(|s| s.authorization(&g.pairing_id))
                            .ok()
                            .map(|(_, p)| {
                                p.unwrap_or(maho_proto::GuestPermissions {
                                    keyboard: true,
                                    mouse: true,
                                    gamepad: true,
                                    clipboard: true,
                                    audio: true,
                                })
                                .into()
                            }),
                        id: g.id,
                        pairing_id: g.pairing_id,
                        name: g.name,
                        trust: format!("{:?}", g.trust),
                        connected_seconds: g.connected_at.elapsed().as_secs(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        pending: registry
            .as_ref()
            .map(|r| {
                r.pending_requests()
                    .into_iter()
                    .map(|p| PendingDto {
                        id: p.id,
                        name: p.name,
                        expires_in_ms: p
                            .expires_at
                            .saturating_duration_since(Instant::now())
                            .as_millis(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        events,
        error: state
            .host_runtime
            .error
            .lock()
            .map_err(|e| e.to_string())?
            .clone(),
    })
}

#[tauri::command]
pub fn host_approve(state: State<'_, AppState>, id: String, owner: bool) -> Result<bool, String> {
    Ok(registry(&state)?.respond(
        &id,
        Some(if owner {
            maho_proto::PairingTrust::Owner
        } else {
            maho_proto::PairingTrust::GuestOnce
        }),
    ))
}

#[tauri::command]
pub fn host_deny(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    Ok(registry(&state)?.respond(&id, None))
}

#[tauri::command]
pub fn host_kick(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    Ok(registry(&state)?.kick(&id))
}

#[tauri::command]
pub fn host_kick_all(state: State<'_, AppState>) -> Result<usize, String> {
    Ok(registry(&state)?.kick_all())
}

#[tauri::command]
pub fn host_guest_permissions(
    state: State<'_, AppState>,
    pairing_id: String,
    permissions: PermissionsDto,
) -> Result<(), String> {
    registry(&state)?
        .set_guest_permissions(
            &pairing_id,
            maho_proto::GuestPermissions {
                keyboard: permissions.keyboard,
                mouse: permissions.mouse,
                gamepad: permissions.gamepad,
                clipboard: permissions.clipboard,
                audio: permissions.audio,
            },
        )
        .map_err(|e| e.to_string())
}

fn magic_packet(mac: &str) -> Result<[u8; 102], String> {
    let parts: Vec<_> = mac.split([':', '-']).collect();
    if parts.len() != 6 || parts.iter().any(|p| p.len() != 2) {
        return Err("MAC must contain six hexadecimal octets".into());
    }
    let mut address = [0; 6];
    for (dst, src) in address.iter_mut().zip(parts) {
        *dst = u8::from_str_radix(src, 16).map_err(|_| "Invalid MAC address")?;
    }
    let mut packet = [255; 102];
    for chunk in packet[6..].chunks_exact_mut(6) {
        chunk.copy_from_slice(&address);
    }
    Ok(packet)
}

/// Subnet-directed broadcast (`ip | !netmask`) for a usable IPv4 interface.
/// Loopback, unspecified, and /31-/32 point-to-point netmasks have none.
fn directed_broadcast(
    ip: std::net::Ipv4Addr,
    netmask: std::net::Ipv4Addr,
) -> Option<std::net::Ipv4Addr> {
    if ip.is_loopback() || ip.is_unspecified() || u32::from(netmask).count_ones() >= 31 {
        return None;
    }
    Some((u32::from(ip) | !u32::from(netmask)).into())
}

/// Limited broadcast plus each IPv4 interface's subnet broadcast, deduplicated.
/// macOS sends 255.255.255.255 out of the default interface only, so the
/// directed address is what reaches a sleeping machine on another LAN interface.
fn wake_targets(interfaces: &[if_addrs::Interface]) -> Vec<std::net::SocketAddrV4> {
    let mut targets = vec![std::net::SocketAddrV4::new(
        std::net::Ipv4Addr::BROADCAST,
        9,
    )];
    for interface in interfaces {
        if let if_addrs::IfAddr::V4(v4) = &interface.addr {
            if let Some(broadcast) = directed_broadcast(v4.ip, v4.netmask) {
                let target = std::net::SocketAddrV4::new(broadcast, 9);
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
        }
    }
    targets
}

#[tauri::command]
pub fn wake_computer(mac: String) -> Result<(), String> {
    let packet = magic_packet(&mac)?;
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    socket.set_broadcast(true).map_err(|e| e.to_string())?;
    let mut errors = Vec::new();
    // Enumeration failure still leaves the limited broadcast to try.
    let interfaces = if_addrs::get_if_addrs().unwrap_or_else(|e| {
        errors.push(format!("interface enumeration: {e}"));
        Vec::new()
    });
    let mut sent = false;
    for target in wake_targets(&interfaces) {
        match socket.send_to(&packet, target) {
            Ok(_) => sent = true,
            Err(e) => errors.push(format!("{target}: {e}")),
        }
    }
    if sent {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wake_packet_has_sixteen_exact_mac_repetitions() {
        let packet = magic_packet("01:23:45:67:89:ab").unwrap();
        assert_eq!(&packet[..6], &[255; 6]);
        for mac in packet[6..].chunks_exact(6) {
            assert_eq!(mac, &[1, 35, 69, 103, 137, 171]);
        }
        assert!(magic_packet("01:23").is_err());
        assert!(magic_packet("01:23:45:67:89:zz").is_err());
    }

    #[test]
    fn wake_directed_broadcast_addresses() {
        use std::net::Ipv4Addr;
        let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
        assert_eq!(
            directed_broadcast(ip("192.168.1.42"), ip("255.255.255.0")),
            Some(ip("192.168.1.255"))
        );
        assert_eq!(
            directed_broadcast(ip("10.20.30.40"), ip("255.255.240.0")),
            Some(ip("10.20.31.255"))
        );
        assert_eq!(directed_broadcast(ip("127.0.0.1"), ip("255.0.0.0")), None);
        assert_eq!(directed_broadcast(ip("0.0.0.0"), ip("0.0.0.0")), None);
        assert_eq!(
            directed_broadcast(ip("100.91.254.71"), ip("255.255.255.255")),
            None
        );
        assert_eq!(
            directed_broadcast(ip("10.0.0.0"), ip("255.255.255.254")),
            None
        );
    }

    #[test]
    fn wake_targets_include_limited_and_unique_subnet_broadcasts() {
        use if_addrs::{IfAddr, Ifv4Addr, Interface};
        use std::net::{Ipv4Addr, SocketAddrV4};
        let v4 = |ip: [u8; 4], mask: [u8; 4]| Interface {
            name: "en0".into(),
            addr: IfAddr::V4(Ifv4Addr {
                ip: ip.into(),
                netmask: mask.into(),
                prefixlen: u32::from(Ipv4Addr::from(mask)).count_ones() as u8,
                broadcast: None,
            }),
            index: None,
            #[cfg(windows)]
            adapter_name: String::new(),
        };
        let targets = wake_targets(&[
            v4([127, 0, 0, 1], [255, 0, 0, 0]),
            v4([192, 168, 1, 42], [255, 255, 255, 0]),
            v4([192, 168, 1, 43], [255, 255, 255, 0]),
            v4([100, 91, 254, 71], [255, 255, 255, 255]),
        ]);
        assert_eq!(
            targets,
            vec![
                SocketAddrV4::new(Ipv4Addr::BROADCAST, 9),
                SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 255), 9),
            ]
        );
    }
}
