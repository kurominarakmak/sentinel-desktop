pub const DEFAULT_GLOBAL_SHORTCUT: &str = "Command+Shift+Space";
pub const PROMPT_WINDOW_LABEL: &str = "prompt";
pub const STATUS_WINDOW_LABEL: &str = "status";
pub const SETTINGS_WINDOW_LABEL: &str = "settings";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    OpenPrompt,
    ShowStatus,
    Settings,
    Quit,
    Ignore,
}

pub fn tray_action(id: &str) -> TrayAction {
    match id {
        "open-prompt" => TrayAction::OpenPrompt,
        "show-status" => TrayAction::ShowStatus,
        "settings" => TrayAction::Settings,
        "quit" => TrayAction::Quit,
        _ => TrayAction::Ignore,
    }
}

pub fn should_hide_on_close(window_label: &str) -> bool {
    window_label == PROMPT_WINDOW_LABEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_prompt_close_requests_hide_the_window() {
        assert!(should_hide_on_close(PROMPT_WINDOW_LABEL));
        assert!(!should_hide_on_close("other"));
    }

    #[test]
    fn tray_quit_is_distinct_from_opening_the_prompt() {
        assert_eq!(tray_action("open-prompt"), TrayAction::OpenPrompt);
        assert_eq!(tray_action("show-status"), TrayAction::ShowStatus);
        assert_eq!(tray_action("settings"), TrayAction::Settings);
        assert_eq!(tray_action("quit"), TrayAction::Quit);
        assert_eq!(tray_action("unknown"), TrayAction::Ignore);
    }

    #[test]
    fn default_shortcut_remains_the_macos_phase_zero_shortcut() {
        assert_eq!(DEFAULT_GLOBAL_SHORTCUT, "Command+Shift+Space");
    }

    #[test]
    fn prompt_window_is_created_hidden_at_startup() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let window = &config["app"]["windows"][0];
        assert_eq!(window["label"], PROMPT_WINDOW_LABEL);
        assert_eq!(window["visible"], false);
    }
}
