use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Application settings that persist across restarts.
///
/// `#[serde(default)]` keeps settings files written by older versions
/// loadable when new fields are added.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub server_http_port: u16,
    pub server_ws_port: u16,
    pub server_udp_port: u16,
    pub bind_address: String,
    pub ui_path: String,
    pub ota_psk: String,
    /// Address the sensing server listens on for ESP32 CSI frames.
    /// `0.0.0.0` accepts boards on the network; `127.0.0.1` is local-only.
    pub udp_bind: String,
    /// Comma-separated IP/CIDR list of sensors allowed to send CSI frames.
    /// Empty means "this Mac's local networks", detected at server start.
    pub udp_allow: String,
    pub auto_discover: bool,
    pub discover_interval_ms: u32,
    pub theme: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            server_http_port: 8080,
            server_ws_port: 8765,
            server_udp_port: 5005,
            bind_address: "127.0.0.1".into(),
            ui_path: String::new(),
            ota_psk: String::new(),
            udp_bind: "0.0.0.0".into(),
            udp_allow: String::new(),
            auto_discover: true,
            discover_interval_ms: 10_000,
            theme: "dark".into(),
        }
    }
}

/// Get the settings file path in the app data directory.
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    // Ensure directory exists
    fs::create_dir_all(&app_dir).map_err(|e| format!("Failed to create app data dir: {}", e))?;

    Ok(app_dir.join("settings.json"))
}

/// Read persisted settings, falling back to defaults if none are saved yet
/// or the file cannot be parsed.
pub fn load_settings(app: &AppHandle) -> AppSettings {
    settings_path(app)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Load settings from disk.
#[tauri::command]
pub async fn get_settings(app: AppHandle) -> Result<Option<AppSettings>, String> {
    let path = settings_path(&app)?;

    if !path.exists() {
        return Ok(None);
    }

    let contents =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read settings: {}", e))?;

    let settings: AppSettings =
        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse settings: {}", e))?;

    Ok(Some(settings))
}

/// Save settings to disk.
#[tauri::command]
pub async fn save_settings(app: AppHandle, settings: AppSettings) -> Result<(), String> {
    let path = settings_path(&app)?;

    let contents = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    fs::write(&path, contents).map_err(|e| format!("Failed to write settings: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = AppSettings::default();
        assert_eq!(settings.server_http_port, 8080);
        assert_eq!(settings.bind_address, "127.0.0.1");
        assert!(settings.auto_discover);
        assert_eq!(settings.udp_bind, "0.0.0.0");
        assert!(settings.udp_allow.is_empty());
    }

    #[test]
    fn test_settings_from_older_version_load_with_defaults() {
        let old = r#"{"server_http_port":9000,"ota_psk":"secret"}"#;
        let settings: AppSettings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.server_http_port, 9000);
        assert_eq!(settings.ota_psk, "secret");
        assert_eq!(settings.udp_bind, "0.0.0.0");
    }

    #[test]
    fn test_settings_serialization() {
        let settings = AppSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        let parsed: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.server_http_port, settings.server_http_port);
    }
}
