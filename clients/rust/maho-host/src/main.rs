use std::io::{self, BufRead, Write};
use std::sync::mpsc;

use anyhow::{bail, Context, Result};
use clap::Parser;
#[cfg(target_os = "macos")]
use maho_host::{accessibility_is_trusted, request_accessibility};
use maho_host::{random_pin, ConsentPrompt, HostConfig, HostServer, PairingStore};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "maho-host",
    version,
    about = "MahoRD v3 screen streaming host",
    long_about = "MahoRD v3 macOS screen streaming host.\n\nFirst run: macOS prompts for Screen Recording and Accessibility. Grant both in System Settings > Privacy & Security, then relaunch the host. Screen Recording is required for capture; Accessibility is required for remote input injection."
)]
struct Cli {
    /// Use this exact 8-digit bootstrap PIN.
    #[arg(long, value_name = "PIN", conflicts_with = "pin")]
    bootstrap_pin: Option<String>,

    /// Generate and display a fresh 8-digit bootstrap PIN (`--pin generate`).
    #[arg(long, value_name = "generate", conflicts_with = "bootstrap_pin")]
    pin: Option<String>,

    /// List persisted paired devices and exit.
    #[arg(long, conflicts_with_all = ["revoke", "bootstrap_pin", "pin"])]
    list_paired: bool,

    /// Revoke one pairing ID and exit.
    #[arg(long, value_name = "ID", conflicts_with_all = ["list_paired", "bootstrap_pin", "pin"])]
    revoke: Option<String>,

    /// LAN HEVC bitrate in Mbps. Accepted range: 50 through 150.
    /// Without this flag the compatibility default is 8 Mbps.
    #[arg(long, value_name = "MBPS", value_parser = clap::value_parser!(u32).range(50..=150))]
    lan_bitrate_mbps: Option<u32>,

    /// Disable ScreenCaptureKit host-audio capture.
    #[arg(long)]
    no_audio: bool,

    /// Automatically approve incoming pairing requests (non-interactive / automation).
    #[arg(long)]
    auto_approve: bool,

    /// Output name to capture (Linux). Auto-detects the focused output when omitted.
    #[arg(long, value_name = "NAME")]
    output: Option<String>,

    /// Register and start the LocalSystem service.
    #[arg(long)]
    install_service: bool,

    /// Stop and remove the LocalSystem service.
    #[arg(long)]
    uninstall_service: bool,

    /// SCM entry point for the LocalSystem service.
    #[arg(long)]
    service_run: bool,

    /// Mark this process as a service-spawned session worker.
    #[arg(long)]
    session_worker: bool,
}

pub fn select_pin<F>(
    bootstrap_pin: Option<String>,
    pin_opt: Option<&str>,
    mut generator: F,
) -> Result<String>
where
    F: FnMut() -> String,
{
    match (bootstrap_pin, pin_opt) {
        (Some(pin), None) => validate_pin(pin),
        (None, Some("generate")) | (None, None) => Ok(generator()),
        (None, Some(other)) => bail!("--pin accepts only 'generate', got '{other}'"),
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts"),
    }
}

#[cfg(target_os = "windows")]
fn init_windows_dpi() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn main() -> Result<()> {
    #[cfg(target_os = "windows")]
    init_windows_dpi();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    #[cfg(target_os = "windows")]
    {
        if cli.install_service {
            maho_host::service_windows::install(&std::env::current_exe()?)?;
            println!("Installed service MahoRDHost");
            return Ok(());
        }
        if cli.uninstall_service {
            maho_host::service_windows::uninstall()?;
            println!("Removed service MahoRDHost");
            return Ok(());
        }
        if cli.service_run {
            maho_host::service_windows::log_to_file();
            maho_host::service_windows::run_service_dispatcher()?;
            return Ok(());
        }
    }

    #[cfg(target_os = "windows")]
    {
        if cli.session_worker {
            maho_host::service_windows::publish_input_desktop();
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if cli.install_service || cli.uninstall_service || cli.service_run {
            bail!("service mode is only supported on Windows");
        }
    }

    #[cfg(target_os = "windows")]
    let store = if cli.session_worker {
        PairingStore::service_default()
    } else {
        PairingStore::host_default()?
    };
    #[cfg(not(target_os = "windows"))]
    let store = PairingStore::host_default()?;

    if cli.list_paired {
        let mut records = store.load_all()?;
        records.sort_by_key(|left| std::cmp::Reverse(left.added_at_unix_ms));
        if records.is_empty() {
            println!("No paired devices.");
        } else {
            for record in records {
                println!(
                    "{}\t{}\t{}",
                    record.id, record.name, record.added_at_unix_ms
                );
            }
        }
        return Ok(());
    }
    if let Some(id) = cli.revoke {
        if store.revoke(&id)? {
            println!("Revoked pairing {id}");
        } else {
            bail!("pairing ID not found: {id}");
        }
        return Ok(());
    }

    let pin = select_pin(cli.bootstrap_pin, cli.pin.as_deref(), random_pin)?;

    onboard_permissions();

    let auto_approve = cli.auto_approve || cli.session_worker;
    let (consent_tx, consent_rx) = mpsc::channel::<ConsentPrompt>();
    std::thread::Builder::new()
        .name("maho-host-consent".into())
        .spawn(move || {
            if auto_approve {
                while let Ok(prompt) = consent_rx.recv() {
                    prompt.respond(true);
                }
            } else {
                consent_loop(consent_rx);
            }
        })
        .context("failed to start consent UI channel")?;

    let mut config = {
        #[cfg(target_os = "macos")]
        {
            HostConfig::macos_default(Some(pin.clone()), store)?
        }
        #[cfg(target_os = "windows")]
        {
            // A worker spawned onto the secure desktop starts before DXGI can
            // describe an output, so startup waits instead of exiting: exiting
            // here makes the service respawn it forever.
            if cli.session_worker {
                maho_host::windows_default_blocking(Some(pin.clone()), store)?
            } else {
                HostConfig::windows_default(Some(pin.clone()), store)?
            }
        }
        #[cfg(target_os = "linux")]
        {
            let monitors = maho_host::probe_hyprland_monitors();
            let output = maho_host::resolve_output_target(
                cli.output.clone(),
                std::env::var("MAHO_OUTPUT").ok(),
                monitors.as_ref(),
            );
            if let Some(name) = &output {
                eprintln!("capturing output: {name}");
            }
            HostConfig::linux_default(Some(pin.clone()), store, output)?
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            bail!(
                "maho-host does not support this platform yet (macOS, Windows and Linux are wired)"
            );
        }
    };
    config.capture_audio = !cli.no_audio;
    config.consent_sender = Some(consent_tx);
    if let Some(mbps) = cli.lan_bitrate_mbps {
        config.bitrate = mbps * 1_000_000;
    }

    let server = HostServer::bind(config)?;
    println!("MahoRD bootstrap PIN: {pin}");
    println!("TCP listening on {}", server.tcp_addr()?);
    println!("UDP listening on {}", server.udp_addr()?);
    println!("Pairing approval requests will appear in this terminal.");
    server.serve()?;
    Ok(())
}

fn validate_pin(pin: String) -> Result<String> {
    if pin.len() != 8 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("bootstrap PIN must be exactly 8 decimal digits");
    }
    Ok(pin)
}

fn consent_loop(receiver: mpsc::Receiver<ConsentPrompt>) {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    while let Ok(prompt) = receiver.recv() {
        print!("Approve pairing for '{}' [y/N]? ", prompt.client_name);
        let _ = io::stdout().flush();
        let approved = lines
            .next()
            .and_then(Result::ok)
            .is_some_and(|line| matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"));
        prompt.respond(approved);
    }
}

#[cfg(target_os = "macos")]
fn onboard_permissions() {
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    let screen_recording = unsafe { CGPreflightScreenCaptureAccess() };
    if !screen_recording {
        eprintln!(
            "Screen Recording permission is required. macOS will prompt now; grant it in System Settings > Privacy & Security > Screen Recording, then relaunch maho-host."
        );
        let _ = unsafe { CGRequestScreenCaptureAccess() };
    }
    if !accessibility_is_trusted() {
        eprintln!(
            "Accessibility permission is required for remote input. macOS will prompt now; enable maho-host (or this terminal) in System Settings > Privacy & Security > Accessibility, then relaunch."
        );
        let _ = request_accessibility();
    }
}

#[cfg(not(target_os = "macos"))]
fn onboard_permissions() {
    #[cfg(target_os = "windows")]
    {
        eprintln!(
            "Windows host: DXGI Desktop Duplication + Media Foundation/NVENC are used; no TCC-style permission prompts are needed."
        );
    }
    #[cfg(target_os = "linux")]
    {
        eprintln!(
            "Linux host: wlroots/Hyprland screencopy capture; VAAPI hardware encoding when available, x264 software otherwise."
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        eprintln!("maho-host capture and input are not wired for this platform yet.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_default_selects_injected_generator_branch() {
        let mut call_count = 0;
        let pin = select_pin(None, None, || {
            call_count += 1;
            "GENERATED_8888".to_string()
        })
        .unwrap();
        assert_eq!(call_count, 1, "default invocation must invoke generator");
        assert_eq!(pin, "GENERATED_8888");
    }

    #[test]
    fn test_pin_generate_flag_selects_injected_generator_branch() {
        let mut call_count = 0;
        let pin = select_pin(None, Some("generate"), || {
            call_count += 1;
            "GENERATED_7777".to_string()
        })
        .unwrap();
        assert_eq!(call_count, 1, "--pin generate must invoke generator");
        assert_eq!(pin, "GENERATED_7777");
    }

    #[test]
    fn test_pin_explicit_bootstrap_pin_bypasses_generator() {
        let mut call_count = 0;
        let pin = select_pin(Some("87654321".into()), None, || {
            call_count += 1;
            "GENERATED_FAIL".to_string()
        })
        .unwrap();
        assert_eq!(
            call_count, 0,
            "explicit bootstrap PIN must bypass generator"
        );
        assert_eq!(pin, "87654321");
    }

    #[test]
    fn test_pin_validation_accepts_8_digits_and_rejects_invalid() {
        assert!(validate_pin("12345678".into()).is_ok());
        assert!(validate_pin("00000000".into()).is_ok());
        assert!(validate_pin("99999999".into()).is_ok());
        assert!(validate_pin("1234567".into()).is_err());
        assert!(validate_pin("123456789".into()).is_err());
        assert!(validate_pin("1234abcd".into()).is_err());
        assert!(validate_pin(" 1234567".into()).is_err());
    }

    #[test]
    fn test_random_pin_retained_and_format_valid() {
        let pin = random_pin();
        assert_eq!(pin.len(), 8);
        assert!(pin.bytes().all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn test_service_cli_flags_parse() {
        let cli = Cli::parse_from([
            "maho-host",
            "--install-service",
            "--uninstall-service",
            "--service-run",
            "--session-worker",
        ]);
        assert!(cli.install_service);
        assert!(cli.uninstall_service);
        assert!(cli.service_run);
        assert!(cli.session_worker);
    }
}
