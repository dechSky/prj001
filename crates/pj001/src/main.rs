use pj001_core::app::{
    self, BlockMode, CommandSpec, Config, InitialLayout, RestoredWindowSpec, SessionSpec,
};
use pj001_core::error::{self, Error};
use pj001_core::render::ThemePalette;
use serde::Deserialize;
use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HELP_TEXT: &str = "pj001 — macOS GPU terminal emulator (wgpu + winit, Rust)

USAGE:
    pj001 [OPTIONS]

OPTIONS:
    -s, --shell <path>          Shell binary path (default: $SHELL or /bin/zsh)
        --theme <name>          Color theme: aurora | obsidian | vellum | holo | bento | crystal
        --block-mode <mode>     Block UI mode: auto (default) | off
    -h, --help                  Print this help and exit
    -V, --version               Print version info and exit

ENV:
    PJ001_NO_BACKDROP=1         Disable macOS NSVisualEffectView vibrancy backdrop
    PJ001_NO_RESTORE=1          Disable session restore for this launch
    PJ001_CONFIG=<path>         Override config file location
    RUST_LOG=<level>            Logging level (info / debug / warn / error)

CONFIG:
    ~/.config/pj001/config.toml — TOML schema:
        [general]   theme = \"...\", shell = \"...\", restore_session = true | false
        [block]     mode = \"auto\" | \"off\"
        [backdrop]  enabled = true | false
        [font]      size = 14.0
        [bell]      visible = true, audible = false

DOCS:
    https://github.com/dechSky/prj001
";

fn version_string() -> String {
    format!(
        "pj001 {} (wgpu {} winit {})",
        env!("CARGO_PKG_VERSION"),
        "29",
        "0.30"
    )
}

fn main() -> error::Result<()> {
    install_panic_hook();
    let _ = env_logger::try_init();
    let args: Vec<String> = std::env::args().collect();
    // CLI --help / --version short-circuit. parse_config 통과시 macOS NSApp init까지 가서
    // user-visible 효과 있을 수 있음 → 일찍 출력 후 exit.
    for a in args.iter().skip(1) {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{HELP_TEXT}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("{}", version_string());
                return Ok(());
            }
            _ => {}
        }
    }
    let file_config = load_user_config_file();
    let config = parse_config(&args[1..], file_config.as_ref())?;
    app::run(config)
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FileConfig {
    #[serde(default)]
    general: FileGeneral,
    #[serde(default)]
    block: FileBlock,
    #[serde(default)]
    backdrop: FileBackdrop,
    #[serde(default)]
    font: FileFont,
    #[serde(default)]
    bell: FileBell,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FileBell {
    /// Visual bell (dock bounce on background). default true.
    #[serde(default)]
    visible: Option<bool>,
    /// Audible bell (NSBeep). default false (macOS Terminal.app 표준).
    #[serde(default)]
    audible: Option<bool>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FileGeneral {
    #[serde(default)]
    theme: Option<String>,
    /// shell 경로 override. CLI --shell이 더 우선.
    #[serde(default)]
    shell: Option<String>,
    /// Restart shells from the last saved window/tab/pane cwd layout. default true.
    #[serde(default)]
    restore_session: Option<bool>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FileBlock {
    /// Block UI render mode. "auto" (default) — OSC 133 수신 시 ON.
    /// "off" — 절대 visual ON 안 함. Phase 4a는 파싱만, visual은 4b부터.
    #[serde(default)]
    mode: Option<String>,
}

/// Phase 3 step 3: macOS NSVisualEffectView 토글.
#[derive(Debug, Deserialize, Default, Clone)]
struct FileBackdrop {
    /// macOS vibrancy backdrop 활성화. None/true = 활성 (default), false = 비활성.
    /// 환경변수 PJ001_NO_BACKDROP=1이 더 우선.
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FileFont {
    /// 폰트 크기(pt). 기본 14.0. CLI override 없음 (config 전용).
    #[serde(default)]
    size: Option<f32>,
}

fn user_config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PJ001_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/pj001/config.toml"))
}

fn load_user_config_file() -> Option<FileConfig> {
    let path = user_config_path()?;
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("config file read failed at {}: {e}", path.display());
            return None;
        }
    };
    match toml::from_str::<FileConfig>(&raw) {
        Ok(cfg) => {
            log::info!("config loaded: {}", path.display());
            Some(cfg)
        }
        Err(e) => {
            log::warn!("config file parse failed at {}: {e}", path.display());
            None
        }
    }
}

fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if let Some(path) = crash_log_path() {
            let backtrace = Backtrace::force_capture().to_string();
            let entry = crash_entry(SystemTime::now(), &info.to_string(), &backtrace);
            let _ = append_crash_entry(&path, &entry);
        }
        previous(info);
    }));
}

fn crash_log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/pj001/crash.log"))
}

fn crash_entry(now: SystemTime, info: &str, backtrace: &str) -> String {
    let ts = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("---\nts_unix: {ts}\npanic: {info}\nbacktrace:\n{backtrace}\n")
}

fn append_crash_entry(path: &Path, entry: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(entry.as_bytes())
}

/// 단일 터미널 모드. `--shell <path>`로 shell 지정, `--theme <name>`로 6 테마 선택.
/// `--block-mode <auto|off>` Block UI 모드 (4a는 파싱만, visual은 4b부터).
/// `file_config` (~/.config/pj001/config.toml)의 general.theme / block.mode는 CLI 미지정 시 사용.
fn parse_config(args: &[String], file_config: Option<&FileConfig>) -> error::Result<Config> {
    let mut shell_override = None;
    let mut cli_shell_override = false;
    let mut theme_name = None;
    let mut block_mode = None;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--shell" | "-s" => {
                cli_shell_override = true;
                shell_override = Some(take_arg_value(a, iter.next())?);
            }
            "--theme" => theme_name = Some(take_arg_value(a, iter.next())?),
            "--block-mode" => block_mode = Some(take_arg_value(a, iter.next())?),
            _ if a.starts_with("--shell=") => {
                cli_shell_override = true;
                shell_override = Some(take_eq_value("--shell", &a["--shell=".len()..])?);
            }
            _ if a.starts_with("--theme=") => {
                theme_name = Some(take_eq_value("--theme", &a["--theme=".len()..])?);
            }
            _ if a.starts_with("--block-mode=") => {
                block_mode = Some(take_eq_value("--block-mode", &a["--block-mode=".len()..])?);
            }
            _ if a.starts_with('-') => {
                return Err(Error::Args(format!("unknown argument: {a}")));
            }
            _ => {}
        }
    }
    let theme = match theme_name.or_else(|| file_config.and_then(|c| c.general.theme.clone())) {
        Some(name) => Some(ThemePalette::by_name(&name).ok_or_else(|| {
            Error::Args(format!(
                "unknown theme: {name} (expected aurora/obsidian/vellum/holo/bento/crystal)"
            ))
        })?),
        None => None,
    };
    // Block mode 파싱 — auto/off만 허용. default "auto". Phase 4b부터 시각 발동.
    let resolved_block_mode = block_mode
        .or_else(|| file_config.and_then(|c| c.block.mode.clone()))
        .unwrap_or_else(|| "auto".to_string());
    let block_mode_enum = match resolved_block_mode.as_str() {
        "auto" => BlockMode::Auto,
        "off" => BlockMode::Off,
        other => {
            return Err(Error::Args(format!(
                "unknown block-mode: {other} (expected auto/off)"
            )));
        }
    };
    log::info!("block-mode: {resolved_block_mode}");
    // shell file_config override (CLI > config). CLI 미지정 + config 있으면 config 사용.
    let resolved_shell =
        shell_override.or_else(|| file_config.and_then(|c| c.general.shell.clone()));
    let mut config = Config::single_shell(resolved_shell).with_block_mode(block_mode_enum);
    if let Some(theme) = theme {
        config = config.with_theme(theme);
    }
    // Phase 3 step 3: [backdrop] enabled. None = default ON. 환경변수가 더 우선.
    if let Some(backdrop) = file_config.and_then(|c| c.backdrop.enabled) {
        config = config.with_backdrop_enabled(Some(backdrop));
    }
    // Phase 3 step 3 후속: [font] size 적용. clamp는 Config 안에서.
    if let Some(size) = file_config.and_then(|c| c.font.size) {
        config = config.with_font_size(Some(size));
        log::info!("font.size={size} applied from config");
    }
    // [bell] visible/audible 적용. default visible=true, audible=false (Terminal.app 표준).
    if let Some(fc) = file_config {
        let visible = fc.bell.visible.unwrap_or(true);
        let audible = fc.bell.audible.unwrap_or(false);
        config = config.with_bell(visible, audible);
        log::info!("bell.visible={visible} bell.audible={audible} (from config)");
    }
    let restore_enabled = file_config
        .and_then(|c| c.general.restore_session)
        .unwrap_or(true)
        && !env_flag_enabled("PJ001_NO_RESTORE");
    if restore_enabled && let Some(path) = session_restore_path() {
        if !cli_shell_override && let Some(state) = load_restore_state(&path) {
            config = apply_restore_state(config, state);
        }
        config = config.with_restore_state_path(Some(path));
    }
    Ok(config)
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| {
            let s = v.trim().to_ascii_lowercase();
            matches!(s.as_str(), "1" | "true" | "yes")
        })
        .unwrap_or(false)
}

fn session_restore_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/pj001/session.toml"))
}

#[derive(Debug, Deserialize)]
struct RestoreStateFile {
    version: u32,
    #[serde(default)]
    windows: Vec<RestoreWindowFile>,
}

#[derive(Debug, Deserialize)]
struct RestoreWindowFile {
    #[serde(default)]
    panes: Vec<RestorePaneFile>,
}

#[derive(Debug, Deserialize)]
struct RestorePaneFile {
    title: Option<String>,
    command: Option<String>,
    cwd: Option<String>,
}

fn load_restore_state(path: &Path) -> Option<RestoreStateFile> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("session restore read failed at {}: {e}", path.display());
            return None;
        }
    };
    let state = match toml::from_str::<RestoreStateFile>(&raw) {
        Ok(state) => state,
        Err(e) => {
            log::warn!("session restore parse failed at {}: {e}", path.display());
            return None;
        }
    };
    if state.version != 1 || state.windows.is_empty() {
        return None;
    }
    Some(state)
}

fn apply_restore_state(mut config: Config, state: RestoreStateFile) -> Config {
    let mut windows = state
        .windows
        .into_iter()
        .filter_map(|window| {
            let panes = window
                .panes
                .into_iter()
                .filter_map(|pane| {
                    let command = pane.command?;
                    Some(SessionSpec {
                        title: pane.title.unwrap_or_else(|| "shell".to_string()),
                        command: CommandSpec::Custom(command),
                        cwd: pane.cwd,
                    })
                })
                .collect::<Vec<_>>();
            if panes.is_empty() {
                None
            } else {
                Some(RestoredWindowSpec { panes })
            }
        })
        .collect::<Vec<_>>();
    if windows.is_empty() {
        return config;
    }
    let first = windows.remove(0);
    config.sessions = first.panes;
    config.initial_layout = InitialLayout::Panes {
        sessions: (0..config.sessions.len()).collect(),
    };
    config = config.with_restored_windows(windows);
    config
}

fn take_arg_value(flag: &str, value: Option<&String>) -> error::Result<String> {
    let Some(value) = value else {
        return Err(error::Error::Args(format!("{flag} requires a value")));
    };
    if value.starts_with("--") {
        return Err(error::Error::Args(format!("{flag} requires a value")));
    }
    Ok(value.clone())
}

fn take_eq_value(flag: &str, value: &str) -> error::Result<String> {
    if value.is_empty() || value.starts_with("--") {
        return Err(error::Error::Args(format!("{flag} requires a value")));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn crash_entry_includes_timestamp_panic_and_backtrace() {
        let entry = crash_entry(
            UNIX_EPOCH + std::time::Duration::from_secs(42),
            "boom",
            "bt",
        );

        assert!(entry.contains("ts_unix: 42"));
        assert!(entry.contains("panic: boom"));
        assert!(entry.contains("backtrace:\nbt"));
    }

    #[test]
    fn append_crash_entry_creates_parent_and_appends() {
        let base = std::env::temp_dir().join(format!(
            "pj001-crash-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = base.join("nested/crash.log");

        append_crash_entry(&path, "one\n").unwrap();
        append_crash_entry(&path, "two\n").unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn parse_default_single_mode() {
        let cfg = parse_config(&args(&[]), None).unwrap();
        assert_eq!(cfg.sessions, Config::single_shell(None).sessions);
        assert!(cfg.restore_state_path.is_some());
    }

    #[test]
    fn parse_single_shell_override() {
        let cfg = parse_config(&args(&["--shell", "/bin/zsh"]), None).unwrap();
        assert_eq!(
            cfg.sessions,
            Config::single_shell(Some("/bin/zsh".to_string())).sessions
        );
        assert!(cfg.restore_state_path.is_some());
    }

    #[test]
    fn parse_rejects_missing_value_before_next_flag() {
        assert!(parse_config(&args(&["--shell", "--theme"]), None).is_err());
        assert!(parse_config(&args(&["--shell=--theme"]), None).is_err());
    }

    #[test]
    fn parse_rejects_unknown_flags() {
        assert!(parse_config(&args(&["--unknown"]), None).is_err());
    }

    #[test]
    fn parse_theme_sets_palette() {
        let cfg = parse_config(&args(&["--theme", "vellum"]), None).unwrap();
        assert_eq!(cfg.theme.map(|t| t.name), Some("vellum"));
    }

    #[test]
    fn parse_theme_eq_form() {
        let cfg = parse_config(&args(&["--theme=aurora"]), None).unwrap();
        assert_eq!(cfg.theme.map(|t| t.name), Some("aurora"));
    }

    #[test]
    fn parse_theme_unknown_rejected() {
        assert!(parse_config(&args(&["--theme", "solarized"]), None).is_err());
    }

    #[test]
    fn parse_no_theme_defaults_to_none() {
        let cfg = parse_config(&args(&[]), None).unwrap();
        assert!(cfg.theme.is_none());
    }

    #[test]
    fn file_config_theme_applied_when_cli_omits() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: Some("vellum".to_string()),
                shell: None,
                restore_session: None,
            },
            block: FileBlock::default(),
            backdrop: FileBackdrop::default(),
            font: FileFont::default(),
            bell: FileBell::default(),
        };
        let cfg = parse_config(&args(&[]), Some(&fc)).unwrap();
        assert_eq!(cfg.theme.map(|t| t.name), Some("vellum"));
    }

    #[test]
    fn cli_theme_overrides_file_config() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: Some("vellum".to_string()),
                shell: None,
                restore_session: None,
            },
            block: FileBlock::default(),
            backdrop: FileBackdrop::default(),
            font: FileFont::default(),
            bell: FileBell::default(),
        };
        let cfg = parse_config(&args(&["--theme", "obsidian"]), Some(&fc)).unwrap();
        assert_eq!(cfg.theme.map(|t| t.name), Some("obsidian"));
    }

    #[test]
    fn file_config_unknown_theme_errors() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: Some("nope".to_string()),
                shell: None,
                restore_session: None,
            },
            block: FileBlock::default(),
            backdrop: FileBackdrop::default(),
            font: FileFont::default(),
            bell: FileBell::default(),
        };
        assert!(parse_config(&args(&[]), Some(&fc)).is_err());
    }

    #[test]
    fn file_config_parses_minimal_toml() {
        let raw = r#"
[general]
theme = "aurora"
"#;
        let parsed: FileConfig = toml::from_str(raw).unwrap();
        assert_eq!(parsed.general.theme.as_deref(), Some("aurora"));
    }

    #[test]
    fn file_config_empty_toml_defaults() {
        let parsed: FileConfig = toml::from_str("").unwrap();
        assert!(parsed.general.theme.is_none());
    }

    #[test]
    fn file_config_unknown_key_ignored() {
        let raw = r#"
[general]
theme = "bento"
[future_section]
something = "value"
"#;
        let parsed: FileConfig = toml::from_str(raw).unwrap();
        assert_eq!(parsed.general.theme.as_deref(), Some("bento"));
    }

    // === Block UI 4a Step 6 — block_mode TOML/CLI ===

    #[test]
    fn block_mode_default_is_auto_no_error() {
        // CLI 미지정 + file 미지정 → "auto" 적용, 에러 없음.
        let cfg = parse_config(&args(&[]), None);
        assert!(cfg.is_ok());
    }

    #[test]
    fn block_mode_cli_accepts_auto_and_off() {
        assert!(parse_config(&args(&["--block-mode", "auto"]), None).is_ok());
        assert!(parse_config(&args(&["--block-mode=off"]), None).is_ok());
    }

    #[test]
    fn block_mode_cli_rejects_invalid_value() {
        assert!(parse_config(&args(&["--block-mode", "magic"]), None).is_err());
        assert!(parse_config(&args(&["--block-mode=neither"]), None).is_err());
    }

    #[test]
    fn block_mode_toml_off_parses() {
        let raw = r#"
[block]
mode = "off"
"#;
        let parsed: FileConfig = toml::from_str(raw).unwrap();
        assert_eq!(parsed.block.mode.as_deref(), Some("off"));
    }

    #[test]
    fn block_mode_toml_invalid_rejected_at_parse_config() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: None,
                shell: None,
                restore_session: None,
            },
            block: FileBlock {
                mode: Some("nonsense".to_string()),
            },
            backdrop: FileBackdrop::default(),
            font: FileFont::default(),
            bell: FileBell::default(),
        };
        assert!(parse_config(&args(&[]), Some(&fc)).is_err());
    }

    #[test]
    fn file_config_backdrop_disabled_applied() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: None,
                shell: None,
                restore_session: None,
            },
            block: FileBlock::default(),
            backdrop: FileBackdrop {
                enabled: Some(false),
            },
            font: FileFont::default(),
            bell: FileBell::default(),
        };
        let cfg = parse_config(&args(&[]), Some(&fc)).unwrap();
        assert_eq!(cfg.backdrop_enabled, Some(false));
    }

    #[test]
    fn file_config_shell_applied_when_cli_omits() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: None,
                shell: Some("/bin/bash".to_string()),
                restore_session: None,
            },
            block: FileBlock::default(),
            backdrop: FileBackdrop::default(),
            font: FileFont::default(),
            bell: FileBell::default(),
        };
        // 단순 parse 성공만 확인 (Config 내부 shell은 SessionSpec 안)
        let _ = parse_config(&args(&[]), Some(&fc)).unwrap();
    }

    #[test]
    fn file_config_font_size_applied() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: None,
                shell: None,
                restore_session: None,
            },
            block: FileBlock::default(),
            backdrop: FileBackdrop::default(),
            font: FileFont { size: Some(18.0) },
            bell: FileBell::default(),
        };
        let cfg = parse_config(&args(&[]), Some(&fc)).unwrap();
        assert_eq!(cfg.font_size, Some(18.0));
    }

    #[test]
    fn file_config_font_size_extreme_values_passthrough_to_clamp() {
        // Codex 권 검증 부족 2: parse는 clamp 안 함 (WindowState init 시 clamp).
        // parse_config는 그대로 전달, clamp는 init 단계.
        for v in [0.0_f32, -5.0, 999.0] {
            let fc = FileConfig {
                general: FileGeneral {
                    theme: None,
                    shell: None,
                    restore_session: None,
                },
                block: FileBlock::default(),
                backdrop: FileBackdrop::default(),
                font: FileFont { size: Some(v) },
                bell: FileBell::default(),
            };
            let cfg = parse_config(&args(&[]), Some(&fc)).unwrap();
            assert_eq!(cfg.font_size, Some(v), "parse_config should not clamp");
        }
    }

    #[test]
    fn full_toml_schema_roundtrip() {
        let raw = r#"
[general]
theme = "obsidian"
shell = "/bin/zsh"
restore_session = true

[block]
mode = "auto"

[backdrop]
enabled = true

[font]
size = 14.0
"#;
        let parsed: FileConfig = toml::from_str(raw).unwrap();
        assert_eq!(parsed.general.theme.as_deref(), Some("obsidian"));
        assert_eq!(parsed.general.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(parsed.general.restore_session, Some(true));
        assert_eq!(parsed.block.mode.as_deref(), Some("auto"));
        assert_eq!(parsed.backdrop.enabled, Some(true));
        assert_eq!(parsed.font.size, Some(14.0));
    }

    #[test]
    fn restore_state_applies_first_window_and_extra_tabs() {
        let state = RestoreStateFile {
            version: 1,
            windows: vec![
                RestoreWindowFile {
                    panes: vec![RestorePaneFile {
                        title: Some("one".to_string()),
                        command: Some("/bin/zsh".to_string()),
                        cwd: Some("/tmp".to_string()),
                    }],
                },
                RestoreWindowFile {
                    panes: vec![RestorePaneFile {
                        title: Some("two".to_string()),
                        command: Some("/bin/bash".to_string()),
                        cwd: Some("/var".to_string()),
                    }],
                },
            ],
        };

        let config = apply_restore_state(Config::single_shell(None), state);

        assert_eq!(config.sessions.len(), 1);
        assert_eq!(config.sessions[0].title, "one");
        assert_eq!(config.sessions[0].cwd.as_deref(), Some("/tmp"));
        assert_eq!(config.restored_windows.len(), 1);
        assert_eq!(config.restored_windows[0].panes[0].title, "two");
    }
}
