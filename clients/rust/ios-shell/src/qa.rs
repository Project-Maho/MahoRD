use maho_app::{PairingRecord, PairingStore};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartupResponse {
    pub host: Option<String>,
    #[serde(default)]
    pub tcp_port: Option<u16>,
    #[serde(default)]
    pub udp_port: Option<u16>,
    #[serde(default)]
    pub pairing_id: Option<String>,
    #[serde(default, alias = "auto_connect")]
    pub auto_connect: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QaProvisioningFile {
    host: String,
    #[serde(default)]
    tcp_port: Option<u16>,
    #[serde(default)]
    udp_port: Option<u16>,
    #[serde(default)]
    pairing_id: Option<String>,
    #[serde(default)]
    pairing: Option<PairingRecord>,
}

pub fn sandbox_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn check_qa_provisioning() -> StartupResponse {
    check_qa_provisioning_in_dir(&sandbox_dir())
}

pub fn check_qa_provisioning_in_dir(base_dir: &std::path::Path) -> StartupResponse {
    #[cfg(debug_assertions)]
    {
        let qa_path = base_dir.join("Documents").join("maho-device-qa.json");
        if qa_path.exists() {
            if let Ok(data) = std::fs::read(&qa_path) {
                if let Ok(qa) = serde_json::from_slice::<QaProvisioningFile>(&data) {
                    let mut imported_id = qa.pairing_id.clone();
                    let mut tcp_port = qa.tcp_port;
                    let mut udp_port = qa.udp_port;

                    if let Some(ref record) = qa.pairing {
                        if imported_id.is_none() {
                            imported_id = Some(record.id.clone());
                        }
                        if tcp_port.is_none() {
                            tcp_port = record.last_endpoint.as_ref().map(|ep| ep.tcp_port);
                        }
                        if udp_port.is_none() {
                            udp_port = record.last_endpoint.as_ref().map(|ep| ep.udp_port);
                        }

                        // Use the SAME production client/Keychain store as reconnect
                        let store = match PairingStore::open_default() {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to open production pairing store for QA provisioning");
                                return StartupResponse::default();
                            }
                        };
                        // Never claim import success after save failure!
                        if let Err(e) = store.save(record.clone()) {
                            tracing::error!(error = %e, id = %record.id, "Failed to save QA pairing record into production store");
                            return StartupResponse::default();
                        }
                        tracing::info!(id = %record.id, "Successfully imported QA pairing record into production pairing store");
                    }

                    let _ = std::fs::remove_file(&qa_path);
                    tracing::info!(host = %qa.host, id = ?imported_id, "QA provisioning consumed and file deleted");
                    return StartupResponse {
                        host: Some(qa.host),
                        tcp_port,
                        udp_port,
                        pairing_id: imported_id,
                        auto_connect: true,
                    };
                } else {
                    tracing::warn!("Failed to parse Documents/maho-device-qa.json");
                }
            }
            let _ = std::fs::remove_file(&qa_path);
        }
    }

    StartupResponse::default()
}
