//! Codex Provider Hub — Tauri application entry.

mod aihub;
mod crypto;
mod cursor;
mod gateway;
mod http_util;
mod sub2api;

use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

fn toggle_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

fn tray_label_from_metrics() -> (String, String) {
    let mut warn = false;
    let mut title = "Hub".to_string();
    let mut tip_parts: Vec<String> = Vec::new();

    match gateway::get_gateway_status() {
        Ok(g) => {
            if !g.running {
                warn = true;
                tip_parts.push("网关: stopped".into());
            } else if !g.healthy {
                warn = true;
                tip_parts.push("网关: unhealthy".into());
            } else {
                tip_parts.push(format!("网关: ok · {} models", g.model_count));
            }
        }
        Err(e) => {
            warn = true;
            tip_parts.push(format!("网关: {e}"));
        }
    }

    match sub2api::fetch_sub2api_usage() {
        Ok(u) => {
            let pct = u.five_hour.remaining_percent;
            title = format!("{pct:.0}%");
            tip_parts.push(format!("Sub2API 5h {pct:.0}% · 7d {:.0}%", u.seven_day.remaining_percent));
            if pct < 20.0 {
                warn = true;
            }
        }
        Err(_) => {
            tip_parts.push("Sub2API: n/a".into());
        }
    }

    match aihub::fetch_aihub_balance() {
        Ok(b) => {
            tip_parts.push(format!("AIHub ${:.2}", b.balance));
            if b.balance < 1.0 {
                warn = true;
            }
        }
        Err(_) => tip_parts.push("AIHub: n/a".into()),
    }

    if warn {
        title = format!("⚠ {title}");
    }
    (title, tip_parts.join(" · "))
}

fn spawn_tray_updater(tray: TrayIcon) {
    std::thread::spawn(move || {
        loop {
            let (title, tip) = tray_label_from_metrics();
            let _ = tray.set_title(Some(&title));
            let _ = tray.set_tooltip(Some(&tip));
            std::thread::sleep(Duration::from_secs(30));
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            gateway::get_gateway_status,
            gateway::start_gateway,
            gateway::stop_gateway,
            gateway::get_provider_config,
            gateway::save_provider_config,
            sub2api::get_sub2api_usage,
            aihub::get_aihub_balance,
            cursor::list_cursor_accounts,
            cursor::add_cursor_account,
            cursor::import_local_cursor_account,
            cursor::remove_cursor_account,
            cursor::get_cursor_usage,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            let show_item = MenuItem::with_id(app, "show", "Show Dashboard", true, None::<&str>)?;
            let refresh_item =
                MenuItem::with_id(app, "refresh", "Refresh Tray Stats", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &refresh_item, &quit_item])?;

            let icon = app
                .default_window_icon()
                .cloned()
                .expect("default window icon required for tray");

            let tray = TrayIconBuilder::with_id("main")
                .icon(icon)
                .menu(&menu)
                .tooltip("Codex Provider Hub")
                .title("…")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => toggle_main_window(app),
                    "refresh" => {
                        if let Some(tray) = app.tray_by_id("main") {
                            let (title, tip) = tray_label_from_metrics();
                            let _ = tray.set_title(Some(&title));
                            let _ = tray.set_tooltip(Some(&tip));
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // Immediate label, then background refresh loop.
            let (title, tip) = tray_label_from_metrics();
            let _ = tray.set_title(Some(&title));
            let _ = tray.set_tooltip(Some(&tip));
            spawn_tray_updater(tray);

            if let Some(window) = app.get_webview_window("main") {
                let window_clone = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window_clone.hide();
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Codex Provider Hub");
}
