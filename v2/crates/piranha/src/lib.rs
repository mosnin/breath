pub mod commands;
pub mod domain;
pub mod menu;
pub mod state;

use commands::{discovery, flash, ota, provision, server, settings, wasm};
use tauri::{Manager, RunEvent, WindowEvent};

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::default())
        .menu(menu::build)
        .on_menu_event(|app, event| menu::on_event(app, event.id().as_ref()))
        .on_window_event(|window, event| {
            // macOS convention: closing the main window hides it; the app
            // keeps running in the Dock until the user quits with Cmd+Q.
            if cfg!(target_os = "macos") && window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Discovery
            discovery::discover_nodes,
            discovery::list_serial_ports,
            discovery::configure_esp32_wifi,
            // Flash
            flash::flash_firmware,
            flash::flash_progress,
            flash::verify_firmware,
            flash::check_espflash,
            flash::supported_chips,
            // OTA
            ota::ota_update,
            ota::batch_ota_update,
            ota::check_ota_endpoint,
            // WASM
            wasm::wasm_list,
            wasm::wasm_upload,
            wasm::wasm_control,
            wasm::wasm_info,
            wasm::wasm_stats,
            wasm::check_wasm_support,
            // Server
            server::start_server,
            server::stop_server,
            server::server_status,
            server::restart_server,
            server::server_logs,
            // Provision
            provision::provision_node,
            provision::read_nvs,
            provision::erase_nvs,
            provision::validate_config,
            provision::generate_mesh_configs,
            // Settings
            settings::get_settings,
            settings::save_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Piranha");

    app.run(|app, event| match event {
        // Clicking the Dock icon brings the hidden main window back.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { has_visible_windows, .. } => {
            if !has_visible_windows {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        }
        // Never leave the sensing-server sidecar running after quit.
        RunEvent::Exit => server::shutdown_server(&app.state::<state::AppState>()),
        _ => {}
    });
}
