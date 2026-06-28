use std::path::Path;

use crate::args::{BeaconStatusJson, ServiceAction, SimpleStatusJson};
use crate::commands::find_index;
use crate::output::format_uptime;

#[cfg(all(feature = "notify", target_os = "linux"))]
pub(crate) fn find_systemctl() -> std::ffi::OsString {
    let usr_bin = std::path::Path::new("/usr/bin/systemctl");
    if usr_bin.exists() {
        return usr_bin.as_os_str().to_os_string();
    }
    let bin = std::path::Path::new("/bin/systemctl");
    if bin.exists() {
        return bin.as_os_str().to_os_string();
    }
    // Fallback to path lookup if neither exists
    std::ffi::OsString::from("systemctl")
}

#[cfg(all(feature = "notify", not(target_os = "linux")))]
#[allow(dead_code)]
pub(crate) fn find_systemctl() -> std::ffi::OsString {
    std::ffi::OsString::from("systemctl")
}

#[cfg(feature = "notify")]
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn handle_service(action: &ServiceAction) -> ix::error::Result<()> {
    if let ServiceAction::Status { path, json } = &action {
        handle_service_status(path.as_deref(), *json);
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        use std::path::PathBuf;

        let home =
            std::env::var("HOME").map_err(|_| ix::error::Error::Config("HOME not set".into()))?;
        let service_dir = PathBuf::from(&home).join(".config/systemd/user");
        let service_file = service_dir.join("ixd.service");

        match action {
            ServiceAction::Install { path } => {
                let watch_path = path.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(&home))
                });
                let watch_path_abs = watch_path.canonicalize().unwrap_or(watch_path);

                std::fs::create_dir_all(&service_dir)?;

                let ix_path = std::env::current_exe()?;
                let daemon_cmd = format!("{} --daemon", ix_path.display());

                let service_content = format!(
                    r"[Unit]
Description=ix background daemon
After=network.target

[Service]
ExecStart={} {}
Restart=on-failure
RestartSec=10
StartLimitBurst=3
StartLimitIntervalSec=60

[Install]
WantedBy=default.target
",
                    daemon_cmd,
                    watch_path_abs.display()
                );

                std::fs::write(&service_file, service_content)?;

                // Reload systemd
                let status = std::process::Command::new(find_systemctl())
                    .args(["--user", "daemon-reload"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "systemctl daemon-reload failed".into(),
                    ));
                }

                println!("ixd service installed at {}", service_file.display());
                println!("Watch path: {}", watch_path_abs.display());
                println!("Run 'ix service start' to start the daemon.");
            }
            ServiceAction::Start => {
                let status = std::process::Command::new(find_systemctl())
                    .args(["--user", "enable", "--now", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to start ixd service".into(),
                    ));
                }
                println!("ixd service started.");
            }
            ServiceAction::Stop => {
                let status = std::process::Command::new(find_systemctl())
                    .args(["--user", "stop", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to stop ixd service".into(),
                    ));
                }
                println!("ixd service stopped.");
            }
            ServiceAction::Restart => {
                let status = std::process::Command::new(find_systemctl())
                    .args(["--user", "restart", "ixd"])
                    .status()?;
                if !status.success() {
                    return Err(ix::error::Error::Config(
                        "Failed to restart ixd service".into(),
                    ));
                }
                println!("ixd service restarted.");
            }
            ServiceAction::Status { .. } => unreachable!(),
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("ix service commands are currently only supported on Linux (systemd).");
        Ok(())
    }
}

#[cfg(not(feature = "notify"))]
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn handle_service(action: &ServiceAction) -> ix::error::Result<()> {
    if let ServiceAction::Status { path, json } = &action {
        handle_service_status(path.as_deref(), *json);
        return Ok(());
    }
    eprintln!("Error: ix service commands require the 'notify' feature.");
    eprintln!("Install with: cargo install moeix --features notify");
    std::process::exit(1);
}

pub(crate) fn handle_service_status(path: Option<&Path>, json: bool) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let search_path = path.unwrap_or(Path::new("."));
    let beacon_opt = find_index(search_path).and_then(|(_, _, beacon)| beacon);

    match beacon_opt {
        Some(beacon) if beacon.is_live() => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let uptime = now.saturating_sub(beacon.start_time);
            if json {
                print_json_status("running", &beacon, Some(uptime));
            } else {
                println!("ixd daemon is running");
                println!("  PID: {}", beacon.pid);
                println!("  Uptime: {}", format_uptime(uptime));
                println!("  Status: {}", beacon.status);
                println!("  Root: {}", beacon.root.display());
                if let Some(ref sock) = beacon.socket_path {
                    println!("  Socket: {}", sock.display());
                }
            }
        }
        Some(beacon) => {
            #[cfg(unix)]
            let is_orphan = {
                use nix::sys::signal::kill;
                use nix::unistd::Pid;
                kill(Pid::from_raw(beacon.pid), None).is_ok()
            };
            #[cfg(not(unix))]
            let is_orphan = false;

            if is_orphan {
                if json {
                    let out = SimpleStatusJson {
                        status: "orphan".to_string(),
                        stale_pid: Some(beacon.pid),
                    };
                    if let Ok(s) = serde_json::to_string(&out) {
                        println!("{s}");
                    }
                } else {
                    println!("PID {} is not ixd (orphan beacon)", beacon.pid);
                }
            } else if json {
                let out = SimpleStatusJson {
                    status: "dead".to_string(),
                    stale_pid: Some(beacon.pid),
                };
                if let Ok(s) = serde_json::to_string(&out) {
                    println!("{s}");
                }
            } else {
                println!(
                    "ixd daemon is not running (stale beacon from PID {})",
                    beacon.pid
                );
            }
        }
        None => {
            if json {
                let out = SimpleStatusJson {
                    status: "not_running".to_string(),
                    stale_pid: None,
                };
                if let Ok(s) = serde_json::to_string(&out) {
                    println!("{s}");
                }
            } else {
                println!("ixd daemon is not running");
            }
        }
    }
}

pub(crate) fn print_json_status(status: &str, beacon: &ix::format::Beacon, uptime: Option<u64>) {
    let json = BeaconStatusJson {
        status: status.to_string(),
        pid: beacon.pid,
        uptime_secs: uptime,
        daemon_status: beacon.status.clone(),
        root: beacon.root.display().to_string(),
        socket: beacon.socket_path.as_ref().map(|p| p.display().to_string()),
        instance_id: beacon.instance_id,
    };
    // Use compact serde_json output to match the original hand-built JSON format.
    // Errors on serialization are extremely unlikely (all fields are simple types)
    // but we fall back to a minimal string to avoid silent output loss.
    match serde_json::to_string(&json) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("ix: warning: JSON serialize error: {e}"),
    }
}
