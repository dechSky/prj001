use pj001_core::app::{self, CommandSpec, Config, QuickSpawnPreset, SessionSpec};
use pj001_core::error::{self, Error};
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
    let config = parse_config(&args[1..])?;
    app::run(config)
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

/// 기존 `--shell` 단일 터미널 모드와 M11-1 `--bridge --left/--right` 모드 지원.
fn parse_config(args: &[String]) -> error::Result<Config> {
    let mut bridge = false;
    let mut shell_override = None;
    let mut left = None;
    let mut right = None;
    let mut saw_bridge_pane_arg = false;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--bridge" => bridge = true,
            "--shell" | "-s" => shell_override = Some(take_arg_value(a, iter.next())?),
            "--left" => {
                saw_bridge_pane_arg = true;
                left = Some(take_arg_value(a, iter.next())?);
            }
            "--right" => {
                saw_bridge_pane_arg = true;
                right = Some(take_arg_value(a, iter.next())?);
            }
            _ if a.starts_with("--shell=") => {
                shell_override = Some(take_eq_value("--shell", &a["--shell=".len()..])?);
            }
            _ if a.starts_with("--left=") => {
                saw_bridge_pane_arg = true;
                left = Some(take_eq_value("--left", &a["--left=".len()..])?);
            }
            _ if a.starts_with("--right=") => {
                saw_bridge_pane_arg = true;
                right = Some(take_eq_value("--right", &a["--right=".len()..])?);
            }
            _ if a.starts_with('-') => {
                return Err(Error::Args(format!("unknown argument: {a}")));
            }
            _ => {}
        }
    }
    if bridge {
        if shell_override.is_some() {
            return Err(error::Error::Args(
                "--bridge cannot be combined with --shell/-s".to_string(),
            ));
        }
        Ok(bridge_config(
            left.unwrap_or_else(|| "claude".to_string()),
            right.unwrap_or_else(|| "codex".to_string()),
        ))
    } else {
        if saw_bridge_pane_arg {
            return Err(error::Error::Args(
                "--left/--right require --bridge".to_string(),
            ));
        }
        Ok(Config::single_shell(shell_override))
    }
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

fn bridge_config(left: String, right: String) -> Config {
    Config::vertical_split(
        SessionSpec {
            title: "Claude".to_string(),
            command: CommandSpec::Custom(left.clone()),
        },
        SessionSpec {
            title: "Codex".to_string(),
            command: CommandSpec::Custom(right.clone()),
        },
    )
    .with_quick_spawn_presets(vec![
        QuickSpawnPreset {
            key: 's',
            spec: SessionSpec {
                title: "shell".to_string(),
                command: CommandSpec::Shell,
            },
        },
        QuickSpawnPreset {
            key: 'c',
            spec: SessionSpec {
                title: "Claude".to_string(),
                command: CommandSpec::Custom(left),
            },
        },
        QuickSpawnPreset {
            key: 'x',
            spec: SessionSpec {
                title: "Codex".to_string(),
                command: CommandSpec::Custom(right),
            },
        },
    ])
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
            parse_config(&args(&[])).unwrap(),
            Config::single_shell(None)
        );
    }

    #[test]
    fn parse_single_shell_override() {
        assert_eq!(
            parse_config(&args(&["--shell", "/bin/zsh"])).unwrap(),
            Config::single_shell(Some("/bin/zsh".to_string()))
        );
    }

    #[test]
    fn parse_bridge_defaults() {
        assert_eq!(
            parse_config(&args(&["--bridge"])).unwrap(),
            bridge_config("claude".to_string(), "codex".to_string())
        );
    }

    #[test]
    fn parse_bridge_custom_commands() {
        assert_eq!(
            parse_config(&args(&[
                "--bridge",
                "--left=/bin/zsh",
                "--right",
                "/bin/bash"
            ]))
            .unwrap(),
            bridge_config("/bin/zsh".to_string(), "/bin/bash".to_string())
        );
    }

    #[test]
    fn parse_bridge_custom_commands_exposes_session_specs() {
        let config = parse_config(&args(&[
            "--bridge",
            "--left=/bin/zsh",
            "--right",
            "/bin/bash",
        ]))
        .unwrap();

        assert_eq!(
            config.sessions,
            vec![
                SessionSpec {
                    title: "Claude".to_string(),
                    command: CommandSpec::Custom("/bin/zsh".to_string()),
                },
                SessionSpec {
                    title: "Codex".to_string(),
                    command: CommandSpec::Custom("/bin/bash".to_string()),
                },
            ]
        );
    }

    #[test]
    fn parse_rejects_missing_value_before_next_flag() {
        assert!(parse_config(&args(&["--shell", "--bridge"])).is_err());
        assert!(parse_config(&args(&["--bridge", "--left", "--right=/bin/zsh"])).is_err());
        assert!(parse_config(&args(&["--shell=--bridge"])).is_err());
    }

    #[test]
    fn parse_rejects_bridge_with_shell_override() {
        assert!(parse_config(&args(&["--bridge", "--shell", "/bin/zsh"])).is_err());
    }

    #[test]
    fn parse_rejects_bridge_pane_args_without_bridge() {
        assert!(parse_config(&args(&["--left", "/bin/zsh"])).is_err());
        assert!(parse_config(&args(&["--right=/bin/zsh"])).is_err());
    }

    #[test]
    fn parse_rejects_unknown_flags() {
        assert!(parse_config(&args(&["--unknown"])).is_err());
    }
}
