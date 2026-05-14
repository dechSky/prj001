use pj001_core::app::{self, Config};
use pj001_core::error::{self, Error};
use pj001_core::render::ThemePalette;
use serde::Deserialize;
use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() -> error::Result<()> {
    install_panic_hook();
    let _ = env_logger::try_init();
    let args: Vec<String> = std::env::args().collect();
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
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FileGeneral {
    #[serde(default)]
    theme: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FileBlock {
    /// Block UI render mode. "auto" (default) — OSC 133 수신 시 ON.
    /// "off" — 절대 visual ON 안 함. Phase 4a는 파싱만, visual은 4b부터.
    #[serde(default)]
    mode: Option<String>,
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
    let mut theme_name = None;
    let mut block_mode = None;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--shell" | "-s" => shell_override = Some(take_arg_value(a, iter.next())?),
            "--theme" => theme_name = Some(take_arg_value(a, iter.next())?),
            "--block-mode" => block_mode = Some(take_arg_value(a, iter.next())?),
            _ if a.starts_with("--shell=") => {
                shell_override = Some(take_eq_value("--shell", &a["--shell=".len()..])?);
            }
            _ if a.starts_with("--theme=") => {
                theme_name = Some(take_eq_value("--theme", &a["--theme=".len()..])?);
            }
            _ if a.starts_with("--block-mode=") => {
                block_mode = Some(take_eq_value(
                    "--block-mode",
                    &a["--block-mode=".len()..],
                )?);
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
    // Block mode 파싱 — auto/off만 허용. default "auto". 4a는 검증만, visual은 4b.
    let resolved_block_mode = block_mode
        .or_else(|| file_config.and_then(|c| c.block.mode.clone()))
        .unwrap_or_else(|| "auto".to_string());
    if resolved_block_mode != "auto" && resolved_block_mode != "off" {
        return Err(Error::Args(format!(
            "unknown block-mode: {resolved_block_mode} (expected auto/off)"
        )));
    }
    log::info!("block-mode: {resolved_block_mode}");
    let config = Config::single_shell(shell_override);
    Ok(match theme {
        Some(theme) => config.with_theme(theme),
        None => config,
    })
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
        assert_eq!(
            parse_config(&args(&[]), None).unwrap(),
            Config::single_shell(None)
        );
    }

    #[test]
    fn parse_single_shell_override() {
        assert_eq!(
            parse_config(&args(&["--shell", "/bin/zsh"]), None).unwrap(),
            Config::single_shell(Some("/bin/zsh".to_string()))
        );
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
            },
            block: FileBlock::default(),
        };
        let cfg = parse_config(&args(&[]), Some(&fc)).unwrap();
        assert_eq!(cfg.theme.map(|t| t.name), Some("vellum"));
    }

    #[test]
    fn cli_theme_overrides_file_config() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: Some("vellum".to_string()),
            },
            block: FileBlock::default(),
        };
        let cfg = parse_config(&args(&["--theme", "obsidian"]), Some(&fc)).unwrap();
        assert_eq!(cfg.theme.map(|t| t.name), Some("obsidian"));
    }

    #[test]
    fn file_config_unknown_theme_errors() {
        let fc = FileConfig {
            general: FileGeneral {
                theme: Some("nope".to_string()),
            },
            block: FileBlock::default(),
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
            general: FileGeneral { theme: None },
            block: FileBlock {
                mode: Some("nonsense".to_string()),
            },
        };
        assert!(parse_config(&args(&[]), Some(&fc)).is_err());
    }
}
