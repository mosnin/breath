//! Native application menu for Piranha.
//!
//! On macOS this produces the standard menu bar (Piranha / File / Edit /
//! View / Window / Help) with the usual system items, plus app-specific
//! entries that navigate the UI. Navigation is delivered to the frontend
//! as a `piranha://navigate` event whose payload is the page id.

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Event emitted to the webview when a menu item asks for a page.
pub const NAVIGATE_EVENT: &str = "piranha://navigate";

/// Menu ids that map to UI pages: (id, label, accelerator).
const PAGES: &[(&str, &str, Option<&str>)] = &[
    ("dashboard", "Dashboard", Some("CmdOrCtrl+1")),
    ("discovery", "Discovery", Some("CmdOrCtrl+2")),
    ("nodes", "Nodes", Some("CmdOrCtrl+3")),
    ("flash", "Flash Firmware", Some("CmdOrCtrl+4")),
    ("ota", "OTA Update", Some("CmdOrCtrl+5")),
    ("wasm", "Edge Modules", Some("CmdOrCtrl+6")),
    ("sensing", "Sensing", Some("CmdOrCtrl+7")),
    ("mesh", "Mesh View", Some("CmdOrCtrl+8")),
];

const NAV_PREFIX: &str = "nav:";

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg = app.package_info();
    let about = AboutMetadata {
        name: Some("Piranha".into()),
        version: Some(pkg.version.to_string()),
        copyright: Some("MIT License. Built on RuView by rUv.".into()),
        comments: Some("WiFi sensing for macOS: presence, vitals and pose from ESP32 CSI nodes.".into()),
        website: Some("https://github.com/ruvnet/RuView".into()),
        website_label: Some("RuView on GitHub".into()),
        ..Default::default()
    };

    let settings = MenuItem::with_id(
        app,
        format!("{NAV_PREFIX}settings"),
        "Settings…",
        true,
        Some("CmdOrCtrl+,"),
    )?;

    let app_menu = Submenu::with_items(
        app,
        "Piranha",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Piranha"), Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some("Hide Piranha"))?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some("Quit Piranha"))?,
        ],
    )?;

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[&PredefinedMenuItem::close_window(app, None)?],
    )?;

    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let go_menu = Submenu::new(app, "Go", true)?;
    for (id, label, accel) in PAGES {
        go_menu.append(&MenuItem::with_id(
            app,
            format!("{NAV_PREFIX}{id}"),
            *label,
            true,
            *accel,
        )?)?;
    }

    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, Some("Zoom"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let help_menu = Submenu::with_items(
        app,
        "Help",
        true,
        &[&MenuItem::with_id(app, "help:docs", "Piranha Help", true, None::<&str>)?],
    )?;

    Menu::with_items(
        app,
        &[
            &app_menu,
            &file_menu,
            &edit_menu,
            &view_menu,
            &go_menu,
            &window_menu,
            &help_menu,
        ],
    )
}

/// Handle a click on one of Piranha's custom menu items.
pub fn on_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if let Some(page) = id.strip_prefix(NAV_PREFIX) {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
        }
        let _ = app.emit(NAVIGATE_EVENT, page);
    } else if id == "help:docs" {
        use tauri_plugin_shell::ShellExt;
        #[allow(deprecated)]
        let _ = app
            .shell()
            .open("https://github.com/ruvnet/RuView/blob/main/docs/user-guide.md", None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ids_are_unique_and_prefixed() {
        let mut ids: Vec<_> = PAGES.iter().map(|(id, _, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PAGES.len());
        assert!(!ids.contains(&"settings"), "settings lives in the app menu");
    }
}
