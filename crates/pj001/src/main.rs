use pj001_core::app::{self, CommandSpec, Config, QuickSpawnPreset, SessionSpec};
use pj001_core::error::{self, Error};

fn main() -> error::Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    let config = parse_config(&args[1..])?;
    app::run(config)
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
