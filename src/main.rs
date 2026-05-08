mod app;
mod error;
mod grid;
mod pty;
mod render;
mod vt;

fn main() -> error::Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    let shell_override = parse_shell_arg(&args[1..]);
    app::run(shell_override)
}

/// `--shell <path>`, `-s <path>`, `--shell=<path>` 형식 지원.
fn parse_shell_arg(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--shell" | "-s" => return iter.next().cloned(),
            _ if a.starts_with("--shell=") => {
                return Some(a["--shell=".len()..].to_string());
            }
            _ => {}
        }
    }
    None
}
