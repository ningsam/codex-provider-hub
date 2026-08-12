//! Codex Provider Hub — Tauri application entry.

mod aihub;
mod channel_switch;
mod codex_sessions;
mod crypto;
mod cursor;
mod gateway;
mod http_util;
mod picker_guard;
mod providers;
mod route_doctor;
mod sub2api;

use parking_lot::Mutex;
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    Manager, PhysicalPosition, Position, Rect, WindowEvent,
};

/// Last known tray icon rect in physical pixels: (x, y, w, h).
static LAST_TRAY_RECT: Mutex<Option<(f64, f64, f64, f64)>> = Mutex::new(None);

fn remember_tray_anchor(window_scale: f64, rect: &Rect, cursor: PhysicalPosition<f64>) {
    let pos = rect.position.to_physical::<f64>(window_scale);
    let size = rect.size.to_physical::<f64>(window_scale);
    let (x, y, w, h) = if size.width > 1.0 && size.height > 1.0 {
        (pos.x, pos.y, size.width, size.height)
    } else {
        // Fallback: treat cursor as a point-sized anchor.
        (cursor.x, cursor.y, 22.0 * window_scale, 22.0 * window_scale)
    };
    *LAST_TRAY_RECT.lock() = Some((x, y, w, h));
}

fn tray_anchor_physical(
    app: &tauri::AppHandle,
    tray: Option<&TrayIcon>,
) -> Option<(f64, f64, f64, f64)> {
    if let Some(cached) = *LAST_TRAY_RECT.lock() {
        return Some(cached);
    }
    let scale = app
        .get_webview_window("main")
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(2.0);
    if let Some(tray) = tray {
        if let Ok(Some(rect)) = tray.rect() {
            let pos = rect.position.to_physical::<f64>(scale);
            let size = rect.size.to_physical::<f64>(scale);
            if size.width > 1.0 && size.height > 1.0 {
                return Some((pos.x, pos.y, size.width, size.height));
            }
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        if let Ok(cursor) = window.cursor_position() {
            let s = 22.0 * scale;
            return Some((cursor.x, cursor.y, s, s));
        }
    }
    None
}

fn position_window_below_tray(app: &tauri::AppHandle, tray: Option<&TrayIcon>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(2.0);
    let outer = window
        .outer_size()
        .unwrap_or(tauri::PhysicalSize::new(1080, 760));
    let win_w = outer.width as f64;
    let win_h = outer.height as f64;
    let gap = 4.0 * scale;

    let (tray_x, tray_y, tray_w, tray_h) = tray_anchor_physical(app, tray).unwrap_or_else(|| {
        // Last resort: top-right-ish of primary work area.
        if let Ok(Some(m)) = window.primary_monitor() {
            let wa = m.work_area();
            let right = wa.position.x as f64 + wa.size.width as f64;
            (
                right - 40.0 * scale,
                wa.position.y as f64,
                22.0 * scale,
                22.0 * scale,
            )
        } else {
            (100.0, 24.0, 22.0, 22.0)
        }
    });

    let monitors = window.available_monitors().unwrap_or_default();
    let monitor = monitors
        .iter()
        .find(|m| {
            let wa = m.work_area();
            let mx = wa.position.x as f64;
            let my = wa.position.y as f64;
            let mw = wa.size.width as f64;
            let mh = wa.size.height as f64;
            // Include menu-bar strip above work area for tray hits.
            let top = m.position().y as f64;
            tray_x >= mx && tray_x < mx + mw && tray_y >= top && tray_y < my + mh
        })
        .cloned()
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());

    let (left, right, top, bottom) = if let Some(m) = monitor {
        let wa = m.work_area();
        let left = wa.position.x as f64;
        let top = wa.position.y as f64;
        let right = left + wa.size.width as f64;
        let bottom = top + wa.size.height as f64;
        (left, right, top, bottom)
    } else {
        (0.0, 1728.0, 0.0, 1117.0)
    };

    // Prefer centering under the tray icon.
    let mut x = tray_x + tray_w / 2.0 - win_w / 2.0;
    // If the icon sits near the right edge, right-align to the icon.
    if tray_x + tray_w > right - win_w * 0.35 {
        x = tray_x + tray_w - win_w;
    }
    if x + win_w > right {
        x = right - win_w;
    }
    if x < left {
        x = left;
    }

    // Sit just below the menu-bar tray icon (or work-area top if tray y is above it).
    let mut y = (tray_y + tray_h + gap).max(top + gap);
    if y + win_h > bottom {
        y = (bottom - win_h).max(top);
    }

    let _ = window.set_position(Position::Physical(PhysicalPosition::new(
        x.round() as i32,
        y.round() as i32,
    )));
}

fn show_main_window(app: &tauri::AppHandle, tray: Option<&TrayIcon>) {
    if let Some(window) = app.get_webview_window("main") {
        position_window_below_tray(app, tray);
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn toggle_main_window(app: &tauri::AppHandle, tray: Option<&TrayIcon>) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            show_main_window(app, tray);
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
            let errored = u.accounts.iter().filter(|a| a.status == "error").count();
            if u.pool_available == 0 {
                warn = true;
                title = if u.pool_total == 0 {
                    "无号".into()
                } else {
                    "封".into()
                };
                tip_parts.push(format!(
                    "OAuth {}/{} · {} 异常",
                    u.pool_available, u.pool_total, errored
                ));
            } else {
                // Tray title prefers the 5h window; fall back to 7d when no
                // account reports an active 5h window.
                let pct = u
                    .five_hour
                    .as_ref()
                    .or(u.seven_day.as_ref())
                    .map(|w| w.remaining_percent);
                title = pct
                    .map(|p| format!("{p:.0}%"))
                    .unwrap_or_else(|| "—".into());
                let fmt_window = |label: &str, w: &Option<sub2api::QuotaWindow>| {
                    w.as_ref()
                        .map(|w| format!("{label} {:.0}%", w.remaining_percent))
                        .unwrap_or_else(|| format!("{label} 无窗口"))
                };
                tip_parts.push(format!(
                    "OAuth {}/{} · {} · {}",
                    u.pool_available,
                    u.pool_total,
                    fmt_window("5h", &u.five_hour),
                    fmt_window("7d", &u.seven_day)
                ));
                if pct.map(|p| p < 20.0).unwrap_or(false) || errored > 0 {
                    warn = true;
                }
            }
            for a in u.accounts.iter().filter(|a| a.status == "error").take(2) {
                let msg = a.error_message.chars().take(48).collect::<String>();
                tip_parts.push(format!(
                    "{}: {}",
                    a.name,
                    if msg.is_empty() { "error" } else { &msg }
                ));
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

fn update_tray_label(tray: &TrayIcon) {
    let (title, tip) = tray_label_from_metrics();
    let _ = tray.set_title(Some(&title));
    let _ = tray.set_tooltip(Some(&tip));
}

fn spawn_tray_updater(tray: TrayIcon) {
    std::thread::spawn(move || loop {
        update_tray_label(&tray);
        std::thread::sleep(Duration::from_secs(30));
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            gateway::get_gateway_status,
            gateway::start_gateway,
            gateway::stop_gateway,
            gateway::get_provider_config,
            gateway::save_provider_config,
            channel_switch::get_channel_switch_status,
            channel_switch::switch_codex_channel,
            channel_switch::restart_codex_app,
            codex_sessions::list_codex_sessions,
            codex_sessions::merge_codex_sessions_into_current_provider,
            sub2api::get_sub2api_usage,
            sub2api::probe_sub2api_official_quota,
            sub2api::set_sub2api_current_account,
            sub2api::recover_sub2api_account,
            sub2api::set_sub2api_auto_pause_threshold,
            sub2api::set_sub2api_routing_policy,
            sub2api::delete_sub2api_account,
            sub2api::import_sub2api_file,
            sub2api::begin_sub2api_browser_login,
            sub2api::get_sub2api_browser_login_status,
            sub2api::complete_sub2api_browser_login,
            sub2api::cancel_sub2api_browser_login,
            aihub::get_aihub_balance,
            aihub::set_aihub_api_key,
            aihub::clear_aihub_api_key,
            cursor::list_cursor_accounts,
            cursor::add_cursor_account,
            cursor::import_local_cursor_account,
            cursor::remove_cursor_account,
            cursor::get_cursor_usage,
            providers::list_providers,
            providers::add_provider,
            providers::remove_provider,
            providers::sync_provider_models,
            providers::probe_provider_models,
            route_doctor::diagnose_sub2api_route,
            route_doctor::probe_sub2api_route_relays,
            route_doctor::repair_sub2api_route,
            picker_guard::get_picker_guard_status,
            picker_guard::apply_picker_guard,
            picker_guard::relaunch_chatgpt_guarded,
            picker_guard::open_chatgpt_guarded,
            picker_guard::set_picker_guard_enabled,
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
                .show_menu_on_left_click(false)
                .tooltip("Codex Provider Hub")
                .title("…")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        let tray = app.tray_by_id("main");
                        show_main_window(app, tray.as_ref());
                    }
                    "refresh" => {
                        if let Some(tray) = app.tray_by_id("main") {
                            std::thread::spawn(move || update_tray_label(&tray));
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    let app = tray.app_handle();
                    let scale = app
                        .get_webview_window("main")
                        .and_then(|w| w.scale_factor().ok())
                        .unwrap_or(2.0);
                    match &event {
                        TrayIconEvent::Click {
                            button,
                            button_state,
                            rect,
                            position,
                            ..
                        } => {
                            remember_tray_anchor(scale, rect, *position);
                            if *button == MouseButton::Left && *button_state == MouseButtonState::Up
                            {
                                toggle_main_window(app, Some(tray));
                            }
                        }
                        TrayIconEvent::Enter { rect, position, .. }
                        | TrayIconEvent::Move { rect, position, .. } => {
                            remember_tray_anchor(scale, rect, *position);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            // Fetch the initial label off the setup thread so tray events stay responsive.
            spawn_tray_updater(tray);

            // Keep ChatGPT Codex model picker from filtering custom slugs.
            picker_guard::spawn_background_loop(app.handle());

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
