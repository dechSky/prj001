# Session Restore Design

## Scope

pj001 restores terminal shape, not live processes.

- Restored: native OS tab count, active window first, pane count, shell command, and each pane cwd.
- Not restored: running process state, scrollback, command output, alternate-screen contents, job control state, pane split ratios, and separate native tab groups.
- Restore starts new shells in the saved cwd. This is the practical first step before persistent PTY or tmux-style process retention.

## State File

Path: `~/.config/pj001/session.toml`

The app periodically writes a small TOML snapshot while windows are open, and also writes when windows are created or closed. If all windows are closed, the state file is removed so the next launch starts clean.

Schema version 1:

```toml
version = 1

[[windows]]
[[windows.panes]]
title = "shell"
command = "/bin/zsh"
cwd = "/Users/derek/project"
```

Current implementation restores all saved `windows` as one native macOS tab group. This matches the current pj001 tab architecture where user-visible tabs are separate `NSWindow`s. Preserving multiple separate tab groups remains a later refinement.

## Config

Session restore is enabled by default. It can be disabled persistently:

```toml
[general]
restore_session = false
```

It can also be disabled for one launch:

```sh
PJ001_NO_RESTORE=1 pj001
```

When `--shell` is supplied, pj001 skips loading the previous session for that launch, but still saves the new session state if restore is enabled. This keeps explicit shell launches predictable.
