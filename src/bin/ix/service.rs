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

#[cfg(feature = "notify")]
fn find_ixd() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let candidate = parent.join("ixd");
        if candidate.exists() {
            return candidate;
        }
    }
    std::path::PathBuf::from("ixd")
}

#[cfg(feature = "notify")]
pub(crate) fn handle_service(action: &ServiceAction) -> ix::error::Result<()> {
    match action {
        ServiceAction::Status { path, json } => {
            handle_service_status(path.as_deref(), *json);
            Ok(())
        }
        ServiceAction::Start { path } => service_start(path.as_deref()),
        ServiceAction::Stop { path } => service_stop(path.as_deref()),
        ServiceAction::Restart { path } => service_restart(path.as_deref()),
        ServiceAction::Install { path } => service_install(path.as_deref()),
    }
}

#[cfg(feature = "notify")]
fn default_ixd_log_path(root: &Path) -> std::path::PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hash = xxhash_rust::xxh64::xxh64(canonical.to_string_lossy().as_bytes(), 0);
    if let Ok(state_home) = std::env::var("XDG_STATE_HOME")
        && !state_home.is_empty()
    {
        return std::path::PathBuf::from(state_home)
            .join("ixd")
            .join(format!("{hash:016x}.log"));
    }
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR")
        && !runtime_dir.is_empty()
    {
        return std::path::PathBuf::from(runtime_dir)
            .join("ixd")
            .join(format!("{hash:016x}.log"));
    }
    #[cfg(unix)]
    // SAFETY: libc::getuid() is a thread-safe POSIX syscall with no preconditions.
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0u32;
    std::env::temp_dir().join(format!("ixd-{uid}-{hash:016x}.log"))
}

#[cfg(feature = "notify")]
fn service_start(path: Option<&Path>) -> ix::error::Result<()> {
    let ixd = find_ixd();
    let watch_path = path.unwrap_or(Path::new("."));
    let log_path = default_ixd_log_path(watch_path);
    let status = std::process::Command::new(&ixd)
        .arg("--daemon")
        .arg("--log-file")
        .arg(&log_path)
        .arg(watch_path)
        .status()
        .map_err(|e| {
            ix::error::Error::Subprocess(format!("failed to execute {}: {e}", ixd.display()))
        })?;
    if !status.success() {
        return Err(ix::error::Error::Subprocess(format!(
            "failed to start ixd daemon (exit {:?}; see log: {})",
            status.code(),
            log_path.display()
        )));
    }
    println!("ixd daemon started for {}.", watch_path.display());
    println!("Log file: {}", log_path.display());
    println!("Run 'ix service status' to verify.");
    Ok(())
}

#[cfg(feature = "notify")]
fn service_stop(path: Option<&Path>) -> ix::error::Result<()> {
    let ixd = find_ixd();
    let stop_path = path.unwrap_or(Path::new("."));
    let status = std::process::Command::new(&ixd)
        .arg("--stop")
        .arg(stop_path)
        .status()
        .map_err(|e| {
            ix::error::Error::Subprocess(format!("failed to execute {}: {e}", ixd.display()))
        })?;
    if !status.success() {
        return Err(ix::error::Error::Subprocess(format!(
            "failed to stop ixd daemon (exit {:?})",
            status.code()
        )));
    }
    Ok(())
}

#[cfg(feature = "notify")]
fn service_restart(path: Option<&Path>) -> ix::error::Result<()> {
    let ixd = find_ixd();
    let target_path = path.unwrap_or(Path::new("."));
    let log_path = default_ixd_log_path(target_path);
    let stop_status = std::process::Command::new(&ixd)
        .arg("--stop")
        .arg(target_path)
        .status()
        .map_err(|e| {
            ix::error::Error::Subprocess(format!("failed to execute {}: {e}", ixd.display()))
        })?;
    if !stop_status.success() {
        return Err(ix::error::Error::Subprocess(format!(
            "failed to stop running ixd daemon during restart (exit {:?})",
            stop_status.code()
        )));
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    let status = std::process::Command::new(&ixd)
        .arg("--daemon")
        .arg("--log-file")
        .arg(&log_path)
        .arg(target_path)
        .status()
        .map_err(|e| {
            ix::error::Error::Subprocess(format!("failed to execute {}: {e}", ixd.display()))
        })?;
    if !status.success() {
        return Err(ix::error::Error::Subprocess(format!(
            "failed to restart ixd daemon (exit {:?}; see log: {})",
            status.code(),
            log_path.display()
        )));
    }
    println!("ixd daemon restarted for {}.", target_path.display());
    println!("Log file: {}", log_path.display());
    Ok(())
}

#[cfg(all(feature = "notify", target_os = "linux"))]
fn escape_systemd_arg(path: &Path) -> ix::error::Result<String> {
    let raw = path.to_str().ok_or_else(|| {
        ix::error::Error::Config(format!(
            "path contains invalid UTF-8 and cannot be written to systemd unit: {}",
            path.display()
        ))
    })?;
    if raw.chars().any(|c| matches!(c, '\n' | '\r' | '\0')) {
        return Err(ix::error::Error::Config(
            "path contains forbidden control characters (newline, CR, or NUL)".into(),
        ));
    }
    let mut escaped = String::with_capacity(raw.len() + 2);
    escaped.push('"');
    for ch in raw.chars() {
        match ch {
            '%' => escaped.push_str("%%"),
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    Ok(escaped)
}

#[cfg(feature = "notify")]
fn service_install(path: Option<&Path>) -> ix::error::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let home =
            std::env::var("HOME").map_err(|_| ix::error::Error::Config("HOME not set".into()))?;
        let service_dir = std::path::PathBuf::from(&home).join(".config/systemd/user");
        let service_file = service_dir.join("ixd.service");

        let watch_path = path.map_or_else(
            || std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(&home)),
            Path::to_path_buf,
        );
        let watch_path_abs = watch_path.canonicalize().unwrap_or(watch_path);

        let ixd_path = find_ixd();
        let ixd_abs = ixd_path.canonicalize().unwrap_or(ixd_path);

        let ixd_quoted = escape_systemd_arg(&ixd_abs)?;
        let watch_quoted = escape_systemd_arg(&watch_path_abs)?;

        std::fs::create_dir_all(&service_dir)?;

        let service_content = format!(
            "[Unit]\n\
Description=ix background daemon\n\
After=network.target\n\
\n\
[Service]\n\
ExecStart={ixd_quoted} {watch_quoted}\n\
Restart=on-failure\n\
RestartSec=10\n\
StartLimitBurst=3\n\
StartLimitIntervalSec=60\n\
\n\
[Install]\n\
WantedBy=default.target\n"
        );

        std::fs::write(&service_file, service_content)?;

        let output = std::process::Command::new(find_systemctl())
            .args(["--user", "daemon-reload"])
            .output();
        match output {
            Ok(out) if out.status.success() => {
                println!("ixd service installed at {}", service_file.display());
                println!("Watch path: {}", watch_path_abs.display());
                println!("Run 'ix service start' to start the daemon.");
            }
            Ok(out) => {
                let stderr_msg = String::from_utf8_lossy(&out.stderr);
                println!("ixd service file written to {}", service_file.display());
                eprintln!(
                    "ix: warning: systemctl --user daemon-reload failed (exit {:?}): {}",
                    out.status.code(),
                    stderr_msg.trim()
                );
                println!("You can run 'ix service start' directly to run ixd natively.");
            }
            Err(e) => {
                println!("ixd service file written to {}", service_file.display());
                eprintln!("ix: warning: failed to execute systemctl --user daemon-reload: {e}");
                println!("You can run 'ix service start' directly to run ixd natively.");
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        println!("Systemd installation is only applicable to Linux.");
        println!("On this platform, run 'ix service start' directly to launch ixd in background.");
        Ok(())
    }
}

#[cfg(not(feature = "notify"))]
pub(crate) fn handle_service(action: &ServiceAction) -> ix::error::Result<()> {
    if let ServiceAction::Status { path, json } = action {
        handle_service_status(path.as_deref(), *json);
        return Ok(());
    }
    Err(ix::error::Error::Config(
        "ix service commands require the 'notify' feature (install with: cargo install ix --features notify)".into(),
    ))
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
                beacon.pid > 0 && kill(Pid::from_raw(beacon.pid), None).is_ok()
            };
            #[cfg(not(unix))]
            let is_orphan = false;

            if is_orphan {
                if json {
                    let out = SimpleStatusJson {
                        status: "orphan".to_string(),
                        stale_pid: Some(beacon.pid),
                    };
                    match serde_json::to_string(&out) {
                        Ok(s) => println!("{s}"),
                        Err(e) => eprintln!("ix: warning: JSON serialize error: {e}"),
                    }
                } else {
                    println!("PID {} is not ixd (orphan beacon)", beacon.pid);
                }
            } else if json {
                let out = SimpleStatusJson {
                    status: "dead".to_string(),
                    stale_pid: Some(beacon.pid),
                };
                match serde_json::to_string(&out) {
                    Ok(s) => println!("{s}"),
                    Err(e) => eprintln!("ix: warning: JSON serialize error: {e}"),
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
                match serde_json::to_string(&out) {
                    Ok(s) => println!("{s}"),
                    Err(e) => eprintln!("ix: warning: JSON serialize error: {e}"),
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
