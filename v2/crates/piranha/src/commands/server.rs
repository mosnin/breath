use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tauri::{AppHandle, Manager, State};

use crate::commands::settings::load_settings;
use crate::state::AppState;

const fn server_binary_name(is_windows: bool) -> &'static str {
    if is_windows {
        "sensing-server.exe"
    } else {
        "sensing-server"
    }
}

const fn path_locator_command(is_windows: bool) -> &'static str {
    if is_windows {
        "where"
    } else {
        "which"
    }
}

/// Default binary name for the sensing server.
const DEFAULT_SERVER_BIN: &str = server_binary_name(cfg!(windows));
const PATH_LOCATOR_COMMAND: &str = path_locator_command(cfg!(windows));

fn first_path_match(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

fn configure_log_level(cmd: &mut Command, log_level: Option<&str>) {
    if let Some(log_level) = log_level {
        cmd.env("RUST_LOG", log_level);
    }
}

/// How the sensing server's UDP CSI receiver is exposed to ESP32 boards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UdpExposure {
    /// Value for `--udp-bind`.
    pub bind: String,
    /// Values for `--udp-allow` (IP or CIDR each).
    pub allow: Vec<String>,
    /// True when the allowlist came from this Mac's network interfaces
    /// rather than from the user's settings.
    pub auto_detected: bool,
}

/// IPv4 subnets of this machine's active, non-loopback interfaces in CIDR
/// form (e.g. `192.168.1.0/24`). Sensors on the same Wi-Fi/LAN as the Mac
/// fall inside one of these.
pub fn local_ipv4_subnets() -> Vec<String> {
    let mut subnets = Vec::new();
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return subnets;
    };
    for iface in ifaces {
        if let if_addrs::IfAddr::V4(v4) = iface.addr {
            if let Some(cidr) = ipv4_subnet(v4.ip, v4.netmask) {
                if !subnets.contains(&cidr) {
                    subnets.push(cidr);
                }
            }
        }
    }
    subnets
}

fn ipv4_subnet(ip: std::net::Ipv4Addr, netmask: std::net::Ipv4Addr) -> Option<String> {
    if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() {
        return None;
    }
    let mask = u32::from(netmask);
    let prefix = mask.count_ones();
    // A non-contiguous or empty mask would make the allowlist match far too much.
    if prefix == 0 || mask.leading_ones() != prefix {
        return None;
    }
    let network = std::net::Ipv4Addr::from(u32::from(ip) & mask);
    Some(format!("{network}/{prefix}"))
}

/// Decide the UDP bind address and source allowlist for the sensing server.
///
/// The server refuses a routable bind without an allowlist (ADR-296), so
/// when the user hasn't listed allowed sensors we allow this Mac's own
/// local subnets. If no subnet can be found we fall back to loopback.
pub fn resolve_udp_exposure(bind: &str, allow: &str, detected: &[String]) -> UdpExposure {
    let bind = match bind.trim() {
        "" => "0.0.0.0".to_string(),
        b => b.to_string(),
    };
    let is_loopback = bind
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false);
    if is_loopback {
        return UdpExposure { bind, allow: Vec::new(), auto_detected: false };
    }

    let explicit: Vec<String> = allow
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if !explicit.is_empty() {
        return UdpExposure { bind, allow: explicit, auto_detected: false };
    }
    if !detected.is_empty() {
        return UdpExposure { bind, allow: detected.to_vec(), auto_detected: true };
    }

    tracing::warn!("No local network found; sensing server will only accept local CSI frames");
    UdpExposure { bind: "127.0.0.1".into(), allow: Vec::new(), auto_detected: false }
}

/// Tauri command: the subnets Piranha would allow automatically right now.
#[tauri::command]
pub async fn detect_lan_subnets() -> Result<Vec<String>, String> {
    Ok(local_ipv4_subnets())
}

/// Find the sensing server binary path.
///
/// Search order:
/// 1. Custom path from config.server_path
/// 2. Bundled in app resources (macOS: Contents/Resources/bin/)
/// 3. Next to the app executable
/// 4. System PATH
fn find_server_binary(app: &AppHandle, custom_path: Option<&str>) -> Result<String, String> {
    // 1. Custom path from settings
    if let Some(path) = custom_path {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    // 2. Bundled in resources (Tauri bundles to Contents/Resources/)
    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir.join("bin").join(DEFAULT_SERVER_BIN);
        if bundled.exists() {
            return Ok(bundled.to_string_lossy().to_string());
        }
        // Also check directly in resources
        let direct = resource_dir.join(DEFAULT_SERVER_BIN);
        if direct.exists() {
            return Ok(direct.to_string_lossy().to_string());
        }
    }

    // 3. Next to the executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let sibling = exe_dir.join(DEFAULT_SERVER_BIN);
            if sibling.exists() {
                return Ok(sibling.to_string_lossy().to_string());
            }
        }
    }

    // 4. Check if it's in PATH
    if let Ok(output) = Command::new(PATH_LOCATOR_COMMAND)
        .arg(DEFAULT_SERVER_BIN)
        .output()
    {
        if output.status.success() {
            if let Some(path) = first_path_match(&output.stdout) {
                return Ok(path);
            }
        }
    }

    Err(format!(
        "Sensing server binary '{}' not found. Please build it with: cargo build --release -p wifi-densepose-sensing-server",
        DEFAULT_SERVER_BIN
    ))
}

/// Start the sensing server as a managed child process.
///
/// The server binary is looked up in the following order:
/// 1. Settings `server_path` if set
/// 2. Bundled resource path
/// 3. Next to executable
/// 4. System PATH
#[tauri::command]
pub async fn start_server(
    app: AppHandle,
    config: ServerConfig,
    state: State<'_, AppState>,
) -> Result<ServerStartResult, String> {
    // Check if already running
    {
        let srv = state.server.lock().map_err(|e| e.to_string())?;
        if srv.running {
            return Err("Server is already running".into());
        }
    }

    // Find server binary
    let server_path = find_server_binary(&app, config.server_path.as_deref())?;

    tracing::info!("Starting sensing server from: {}", server_path);

    // Build command with configuration
    let mut cmd = Command::new(&server_path);

    if let Some(port) = config.http_port {
        cmd.args(["--http-port", &port.to_string()]);
    }
    if let Some(port) = config.ws_port {
        cmd.args(["--ws-port", &port.to_string()]);
    }
    if let Some(port) = config.udp_port {
        cmd.args(["--udp-port", &port.to_string()]);
    }
    if let Some(ref bind_addr) = config.bind_address {
        cmd.args(["--bind", bind_addr]);
    }
    configure_log_level(&mut cmd, config.log_level.as_deref());

    // Let ESP32 boards on this Mac's network reach the UDP CSI receiver.
    let settings = load_settings(&app);
    let udp = resolve_udp_exposure(
        config.udp_bind.as_deref().unwrap_or(&settings.udp_bind),
        config.udp_allow.as_deref().unwrap_or(&settings.udp_allow),
        &local_ipv4_subnets(),
    );
    cmd.args(["--udp-bind", &udp.bind]);
    if !udp.allow.is_empty() {
        cmd.args(["--udp-allow", &udp.allow.join(",")]);
    }
    tracing::info!("Sensor UDP: bind {} allow {:?}", udp.bind, udp.allow);

    // Piranha defaults to real ESP32 hardware. Demo data is only used when
    // the user explicitly picks "simulate", which the sensing-server tags
    // as synthetic in its API responses.
    let source = config.source.as_deref().unwrap_or("esp32");
    cmd.args(["--source", source]);

    // Redirect stdout/stderr to pipes for monitoring
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Spawn the child process
    let child = cmd.spawn().map_err(|e| {
        format!(
            "Failed to start server: {}. Is '{}' installed?",
            e, server_path
        )
    })?;

    let pid = child.id();

    // Store the child process in state
    {
        let mut srv = state.server.lock().map_err(|e| e.to_string())?;
        srv.running = true;
        srv.pid = Some(pid);
        srv.http_port = config.http_port;
        srv.ws_port = config.ws_port;
        srv.udp_port = config.udp_port;
        srv.child = Some(child);
    }

    tracing::info!("Started sensing server with PID {}", pid);

    Ok(ServerStartResult {
        pid,
        http_port: config.http_port,
        ws_port: config.ws_port,
        udp_port: config.udp_port,
        udp,
    })
}

/// Stop the managed sensing server process.
///
/// First attempts graceful termination (SIGTERM), then SIGKILL after timeout.
#[tauri::command]
pub async fn stop_server(state: State<'_, AppState>) -> Result<(), String> {
    // Extract child process and take ownership for killing
    let (child_id, mut child_process) = {
        let mut srv = state.server.lock().map_err(|e| e.to_string())?;
        if !srv.running {
            return Err("Server is not running".into());
        }
        let pid = srv.pid;
        let child = srv.child.take(); // Take ownership of child
        (pid, child)
    };

    let child_id = match child_id {
        Some(id) => id,
        None => return Err("No server process found".into()),
    };

    tracing::info!("Stopping sensing server with PID {}", child_id);

    // First try graceful termination via SIGTERM
    #[cfg(unix)]
    {
        unsafe {
            // Kill the process group (negative PID) to kill all children too
            let _ = libc::kill(-(child_id as i32), libc::SIGTERM);
            // Also kill the main process directly
            let _ = libc::kill(child_id as i32, libc::SIGTERM);
        }
    }

    // Wait briefly for graceful shutdown
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Check if still running
    let still_running = {
        let mut sys = System::new();
        let pid = Pid::from_u32(child_id);
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        sys.process(pid).is_some()
    };

    // Force kill if still running
    if still_running {
        tracing::warn!("Server still running after SIGTERM, sending SIGKILL");

        #[cfg(unix)]
        {
            unsafe {
                // SIGKILL the process group and main process
                let _ = libc::kill(-(child_id as i32), libc::SIGKILL);
                let _ = libc::kill(child_id as i32, libc::SIGKILL);
            }
        }

        // Also use the child handle if available
        if let Some(ref mut child) = child_process {
            let _ = child.kill();
        }
    }

    // Wait for process to actually terminate
    if let Some(ref mut child) = child_process {
        let _ = child.wait();
    }

    // Final verification and cleanup
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Clear state
    {
        let mut srv = state.server.lock().map_err(|e| e.to_string())?;
        srv.running = false;
        srv.pid = None;
        srv.http_port = None;
        srv.ws_port = None;
        srv.udp_port = None;
        srv.child = None;
    }

    // Verify process is dead
    let still_alive = {
        let mut sys = System::new();
        let pid = Pid::from_u32(child_id);
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        sys.process(pid).is_some()
    };

    if still_alive {
        tracing::error!("Failed to kill server process {}", child_id);
        return Err(format!("Failed to stop server process {}", child_id));
    }

    tracing::info!("Stopped sensing server");

    Ok(())
}

/// Synchronously terminate the managed sensing server, if any.
///
/// Called when Piranha quits so the sidecar never outlives the app.
pub fn shutdown_server(state: &AppState) {
    let (pid, child) = match state.server.lock() {
        Ok(mut srv) => {
            let pid = srv.pid.take();
            let child = srv.child.take();
            srv.running = false;
            srv.http_port = None;
            srv.ws_port = None;
            srv.udp_port = None;
            (pid, child)
        }
        Err(_) => return,
    };

    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGTERM);
            let _ = libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;

    if let Some(mut child) = child {
        for _ in 0..10 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Get sensing server status including resource usage.
#[tauri::command]
pub async fn server_status(state: State<'_, AppState>) -> Result<ServerStatusResponse, String> {
    let srv = state.server.lock().map_err(|e| e.to_string())?;

    if !srv.running || srv.pid.is_none() {
        return Ok(ServerStatusResponse {
            running: false,
            pid: None,
            http_port: None,
            ws_port: None,
            udp_port: None,
            memory_mb: None,
            cpu_percent: None,
            uptime_secs: None,
        });
    }

    // srv.pid.is_none() is checked above; the expect is unreachable in practice.
    let pid = srv.pid.expect("pid checked as Some before this point");
    let mut sys = System::new();
    let sysinfo_pid = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[sysinfo_pid]), true);

    let (memory_mb, cpu_percent) = sys
        .process(sysinfo_pid)
        .map(|proc| {
            let mem = proc.memory() as f64 / 1024.0 / 1024.0;
            let cpu = proc.cpu_usage();
            (Some(mem), Some(cpu))
        })
        .unwrap_or((None, None));

    // Calculate uptime if we have start time
    let uptime_secs = srv
        .start_time
        .map(|start| std::time::Instant::now().duration_since(start).as_secs());

    Ok(ServerStatusResponse {
        running: srv.running,
        pid: Some(pid),
        http_port: srv.http_port,
        ws_port: srv.ws_port,
        udp_port: srv.udp_port,
        memory_mb,
        cpu_percent,
        uptime_secs,
    })
}

/// Restart the sensing server with the same or new configuration.
#[tauri::command]
pub async fn restart_server(
    app: AppHandle,
    config: Option<ServerConfig>,
    state: State<'_, AppState>,
) -> Result<ServerStartResult, String> {
    // Get current config if no new config provided
    let restart_config = if let Some(cfg) = config {
        cfg
    } else {
        let srv = state.server.lock().map_err(|e| e.to_string())?;
        ServerConfig {
            http_port: srv.http_port,
            ws_port: srv.ws_port,
            udp_port: srv.udp_port,
            log_level: None,
            bind_address: None,
            server_path: None,
            source: None, // Falls through to the "esp32" default.
            udp_bind: None,
            udp_allow: None,
        }
    };

    // Stop existing server
    let _ = stop_server(state.clone()).await;

    // Brief delay to ensure port is released
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Start with new config
    start_server(app, restart_config, state).await
}

/// Get server logs (last N lines from stdout/stderr).
#[tauri::command]
pub async fn server_logs(
    _lines: Option<usize>,
    state: State<'_, AppState>,
) -> Result<ServerLogsResponse, String> {
    let _srv = state.server.lock().map_err(|e| e.to_string())?;

    // For now, return empty logs - full implementation would capture stdout/stderr
    // to ring buffer during process lifetime
    Ok(ServerLogsResponse {
        stdout: Vec::new(),
        stderr: Vec::new(),
        truncated: false,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub http_port: Option<u16>,
    pub ws_port: Option<u16>,
    pub udp_port: Option<u16>,
    pub log_level: Option<String>,
    pub bind_address: Option<String>,
    pub server_path: Option<String>,
    /// Data source: "auto", "wifi", "esp32", "simulate"
    pub source: Option<String>,
    /// Overrides the `udp_bind` setting for this start.
    #[serde(default)]
    pub udp_bind: Option<String>,
    /// Overrides the `udp_allow` setting for this start.
    #[serde(default)]
    pub udp_allow: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerStartResult {
    pub pid: u32,
    pub http_port: Option<u16>,
    pub ws_port: Option<u16>,
    pub udp_port: Option<u16>,
    pub udp: UdpExposure,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerStatusResponse {
    pub running: bool,
    pub pid: Option<u32>,
    pub http_port: Option<u16>,
    pub ws_port: Option<u16>,
    pub udp_port: Option<u16>,
    pub memory_mb: Option<f64>,
    pub cpu_percent: Option<f32>,
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerLogsResponse {
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_pass_log_level_through_rust_log() {
        let mut command = Command::new("sensing-server");
        configure_log_level(&mut command, Some("debug"));

        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let rust_log = command
            .get_envs()
            .find(|(key, _)| key.to_string_lossy() == "RUST_LOG")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());

        assert!(!args.iter().any(|arg| arg == "--log-level"));
        assert_eq!(rust_log.as_deref(), Some("debug"));
    }

    #[test]
    fn should_use_platform_binary_names() {
        assert_eq!(server_binary_name(true), "sensing-server.exe");
        assert_eq!(path_locator_command(true), "where");
        assert_eq!(server_binary_name(false), "sensing-server");
        assert_eq!(path_locator_command(false), "which");
    }

    #[test]
    fn should_use_first_path_match_when_locator_returns_multiple_results() {
        let matches = b"C:\\first\\sensing-server.exe\r\nC:\\second\\sensing-server.exe\r\n";

        assert_eq!(
            first_path_match(matches).as_deref(),
            Some("C:\\first\\sensing-server.exe")
        );
        assert_eq!(first_path_match(b"\r\n\r\n"), None);
    }

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig {
            http_port: Some(8080),
            ws_port: Some(8765),
            udp_port: Some(5005),
            log_level: None,
            bind_address: None,
            server_path: None,
            source: Some("simulate".to_string()),
            udp_bind: None,
            udp_allow: None,
        };

        assert_eq!(config.http_port, Some(8080));
        assert_eq!(config.ws_port, Some(8765));
    }

    #[test]
    fn subnet_from_interface_address() {
        let ip = "192.168.1.42".parse().unwrap();
        assert_eq!(
            ipv4_subnet(ip, "255.255.255.0".parse().unwrap()).as_deref(),
            Some("192.168.1.0/24")
        );
        assert_eq!(
            ipv4_subnet("10.20.30.40".parse().unwrap(), "255.255.0.0".parse().unwrap()).as_deref(),
            Some("10.20.0.0/16")
        );
        // Loopback, link-local and degenerate masks are never allowlisted.
        assert_eq!(ipv4_subnet("127.0.0.1".parse().unwrap(), "255.0.0.0".parse().unwrap()), None);
        assert_eq!(ipv4_subnet("169.254.3.4".parse().unwrap(), "255.255.0.0".parse().unwrap()), None);
        assert_eq!(ipv4_subnet(ip, "0.0.0.0".parse().unwrap()), None);
        assert_eq!(ipv4_subnet(ip, "255.0.255.0".parse().unwrap()), None);
    }

    #[test]
    fn udp_exposure_defaults_to_local_subnets() {
        let detected = vec!["192.168.1.0/24".to_string()];
        let udp = resolve_udp_exposure("0.0.0.0", "", &detected);
        assert_eq!(udp.bind, "0.0.0.0");
        assert_eq!(udp.allow, detected);
        assert!(udp.auto_detected);

        // Empty bind means the network default too.
        assert_eq!(resolve_udp_exposure("", "", &detected).bind, "0.0.0.0");
    }

    #[test]
    fn udp_exposure_prefers_user_allowlist() {
        let udp = resolve_udp_exposure("0.0.0.0", " 10.0.0.5, 10.1.0.0/16 ,", &["192.168.1.0/24".into()]);
        assert_eq!(udp.allow, vec!["10.0.0.5", "10.1.0.0/16"]);
        assert!(!udp.auto_detected);
    }

    #[test]
    fn udp_exposure_loopback_and_fallback() {
        let local = resolve_udp_exposure("127.0.0.1", "10.0.0.5", &["192.168.1.0/24".into()]);
        assert_eq!(local.bind, "127.0.0.1");
        assert!(local.allow.is_empty());

        // No network and no allowlist: the server would refuse a routable bind,
        // so stay on loopback instead of failing to start.
        let none = resolve_udp_exposure("0.0.0.0", "", &[]);
        assert_eq!(none.bind, "127.0.0.1");
        assert!(none.allow.is_empty());
    }
}
