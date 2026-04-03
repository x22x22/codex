use codex_terminal_detection::TerminalInfo;
use codex_terminal_detection::TerminalName;
use codex_terminal_detection::terminal_info;

/// Returns whether Codex should enable progressive keyboard enhancement flags.
///
/// We currently disable them for the VS Code integrated terminal under WSL
/// because that terminal stack partially supports the protocol and breaks
/// dead-key composition for some keyboard layouts.
pub fn should_enable_keyboard_enhancement() -> bool {
    should_enable_keyboard_enhancement_for(terminal_info(), process_is_wsl())
}

/// Pure-logic helper for [`should_enable_keyboard_enhancement`], separated so
/// tests can inject terminal identity and WSL status without touching the
/// process environment or filesystem.
///
/// The rule is a denylist: disable enhancement only for known-broken
/// combinations. Today, the only entry is VS Code under WSL, where the
/// terminal's partial protocol support corrupts dead-key composition sequences
/// for layouts that rely on them (e.g. Portuguese, French, Spanish).
fn should_enable_keyboard_enhancement_for(terminal: TerminalInfo, is_wsl: bool) -> bool {
    !(is_wsl && matches!(terminal.name, TerminalName::VsCode))
}

/// Detects whether the current process is running inside Windows Subsystem for Linux.
///
/// Uses a three-tier heuristic (checked only on Linux; returns `false` on all other targets):
/// 1. `WSL_DISTRO_NAME` environment variable (set by WSL 2).
/// 2. `WSL_INTEROP` environment variable (set by WSL 1 and some WSL 2 configurations).
/// 3. `/proc/version` containing "microsoft" (fallback for minimal WSL images that
///    strip environment variables).
fn process_is_wsl() -> bool {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::env::var_os("WSL_INTEROP").is_some()
        {
            return true;
        }
        match std::fs::read_to_string("/proc/version") {
            Ok(version) => version.to_lowercase().contains("microsoft"),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disables_keyboard_enhancement_for_vscode_under_wsl() {
        assert!(!should_enable_keyboard_enhancement_for(
            TerminalInfo {
                name: TerminalName::VsCode,
                term_program: None,
                version: None,
                term: None,
                multiplexer: None,
            },
            /*is_wsl*/ true,
        ));
    }

    #[test]
    fn keeps_keyboard_enhancement_enabled_outside_wsl_vscode() {
        assert!(should_enable_keyboard_enhancement_for(
            TerminalInfo {
                name: TerminalName::VsCode,
                term_program: None,
                version: None,
                term: None,
                multiplexer: None,
            },
            /*is_wsl*/ false,
        ));

        assert!(should_enable_keyboard_enhancement_for(
            TerminalInfo {
                name: TerminalName::AppleTerminal,
                term_program: None,
                version: None,
                term: None,
                multiplexer: None,
            },
            /*is_wsl*/ true,
        ));
    }
}
