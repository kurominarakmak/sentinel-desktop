mod windowing;

use sentinel_agent_api::{detect_installation, AgentEvent, AgentKind, InstallationStatus};
use serde::Serialize;
use std::time::Duration;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use windowing::{
    should_hide_on_close, tray_action, TrayAction, DEFAULT_GLOBAL_SHORTCUT, PROMPT_WINDOW_LABEL,
};

#[cfg(target_os = "macos")]
const MACOS_ACTIVATION_POLICY: tauri::ActivationPolicy = tauri::ActivationPolicy::Accessory;

#[derive(Serialize)]
struct Diagnostics {
    fake: String,
    codex: String,
    claude_code: String,
}

// Tauri requires the returned handle to outlive setup for the tray item to remain visible.
#[allow(dead_code)]
struct TrayState<R: tauri::Runtime>(TrayIcon<R>);

#[tauri::command]
fn spike_diagnostics() -> Diagnostics {
    Diagnostics {
        fake: "available (built-in deterministic sequence)".into(),
        codex: format_installation(detect_installation(AgentKind::Codex)),
        claude_code: format_installation(detect_installation(AgentKind::ClaudeCode)),
    }
}

#[tauri::command]
fn run_fake_agent(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        for event in sentinel_fake_agent::deterministic_events() {
            let state = match &event {
                AgentEvent::Completed => "Completed",
                AgentEvent::Failed { .. } => "Failed",
                AgentEvent::WaitingForInput => "Waiting",
                _ => "Running",
            };
            let _ = app.emit("agent-event", &event);
            if let Some(tray) = app.tray_by_id("spike-tray") {
                let _ = tray.set_title(Some(state));
            }
            tokio::time::sleep(Duration::from_millis(350)).await;
        }
    });
}

#[tauri::command]
fn record_manual_probe_request(app: AppHandle, agent: String) {
    let event = AgentEvent::Message {
        text: format!("{agent} probe is manual-only; no model request was started."),
    };
    let _ = app.emit("agent-event", event);
}

fn format_installation(status: InstallationStatus) -> String {
    match status {
        InstallationStatus::Available { version, .. } => format!("available: {version}"),
        InstallationStatus::NotInstalled => "not_installed".into(),
        InstallationStatus::Unusable { detail } => format!("unusable: {detail}"),
    }
}

fn show_prompt(app: &AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window(PROMPT_WINDOW_LABEL)
        .ok_or_else(|| tauri::Error::AssetNotFound(PROMPT_WINDOW_LABEL.into()))?;
    window.show()?;
    window.unminimize()?;
    // Tauri can center the prompt on its current display, but cannot reliably discover the
    // display of another application's focused window on every platform.
    window.center()?;
    window.set_focus()?;
    app.emit("focus-task-input", ())?;
    Ok(())
}

fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    eprintln!("agent-sentinel: tray setup started");
    let icon = match Image::from_bytes(include_bytes!("../icons/tray-template.png")) {
        Ok(icon) => {
            eprintln!("agent-sentinel: tray icon loaded");
            icon
        }
        Err(error) => {
            eprintln!("agent-sentinel: tray icon load failed: {error:?}");
            return Err(error);
        }
    };
    let open = MenuItem::with_id(app, "open-prompt", "Open Prompt", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "show-status", "Show Spike Status", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &status, &quit])?;
    let builder = TrayIconBuilder::with_id("spike-tray")
        .icon(icon)
        .menu(&menu)
        .tooltip("Agent Sentinel")
        .on_menu_event(|app, event| match tray_action(event.id.as_ref()) {
            TrayAction::OpenPrompt | TrayAction::ShowSpikeStatus => {
                let _ = show_prompt(app);
            }
            TrayAction::Quit => app.exit(0),
            TrayAction::Ignore => {}
        });
    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);
    let tray = match builder.build(app) {
        Ok(tray) => {
            eprintln!("agent-sentinel: tray successfully built");
            tray
        }
        Err(error) => {
            eprintln!("agent-sentinel: tray build failed: {error:?}");
            return Err(error);
        }
    };
    app.manage(TrayState(tray));
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(MACOS_ACTIVATION_POLICY);
                app.set_dock_visibility(false);
            }
            if let Err(error) = install_tray(app.handle()) {
                eprintln!("agent-sentinel: tray setup failed: {error:?}");
                return Err(Box::new(error));
            }
            app.global_shortcut()
                .on_shortcut(DEFAULT_GLOBAL_SHORTCUT, |app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        let _ = show_prompt(app);
                    }
                })?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if should_hide_on_close(window.label()) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            spike_diagnostics,
            run_fake_agent,
            record_manual_probe_request
        ])
        .run(tauri::generate_context!())
        .expect("Tauri spike failed to run");
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn startup_uses_the_menu_bar_accessory_policy() {
        assert!(matches!(
            MACOS_ACTIVATION_POLICY,
            tauri::ActivationPolicy::Accessory
        ));
    }
}
