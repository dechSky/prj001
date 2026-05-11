use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use winit::event_loop::EventLoopProxy;

use crate::app::event::{PaneId, UserEvent};
use crate::grid::Term;
use crate::vt::TermPerform;

pub fn spawn(
    mut reader: Box<dyn Read + Send>,
    term: Arc<Mutex<Term>>,
    proxy: EventLoopProxy<UserEvent>,
    pane: PaneId,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            let mut parser = vte::Parser::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = proxy.send_event(UserEvent::ChildExited { pane, code: 0 });
                        return;
                    }
                    Ok(n) => {
                        if log::log_enabled!(log::Level::Debug) {
                            log::debug!("pty-rx ({n}B): {}", hex_ascii(&buf[..n]));
                        }
                        {
                            let mut term = term.lock().unwrap();
                            let mut perform = TermPerform::new(&mut term);
                            parser.advance(&mut perform, &buf[..n]);
                        }
                        let _ = proxy.send_event(UserEvent::Repaint(pane));
                    }
                    Err(e) => {
                        let _ = proxy.send_event(UserEvent::PtyError {
                            pane,
                            message: e.to_string(),
                        });
                        return;
                    }
                }
            }
        })
        .expect("spawn pty-reader")
}

/// Hex + ASCII 형식: 한 byte당 hex 2자리 + 인쇄 가능 ASCII만 별도 prefix.
/// 디버깅용. 길면 truncate.
fn hex_ascii(bytes: &[u8]) -> String {
    const MAX: usize = 256;
    let slice = if bytes.len() > MAX {
        &bytes[..MAX]
    } else {
        bytes
    };
    let mut hex = String::with_capacity(slice.len() * 3);
    let mut ascii = String::with_capacity(slice.len());
    for &b in slice {
        hex.push_str(&format!("{:02x} ", b));
        ascii.push(if (0x20..=0x7e).contains(&b) {
            b as char
        } else {
            '.'
        });
    }
    let truncated = if bytes.len() > MAX { "..(trunc)" } else { "" };
    format!("[{}] '{}'{}", hex.trim_end(), ascii, truncated)
}
