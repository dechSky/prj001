use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use winit::event_loop::EventLoopProxy;

use crate::app::event::UserEvent;
use crate::grid::Term;
use crate::vt::TermPerform;

pub fn spawn(
    mut reader: Box<dyn Read + Send>,
    term: Arc<Mutex<Term>>,
    proxy: EventLoopProxy<UserEvent>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            let mut parser = vte::Parser::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = proxy.send_event(UserEvent::ChildExited(0));
                        return;
                    }
                    Ok(n) => {
                        {
                            let mut term = term.lock().unwrap();
                            let mut perform = TermPerform::new(&mut term);
                            parser.advance(&mut perform, &buf[..n]);
                        }
                        let _ = proxy.send_event(UserEvent::Repaint);
                    }
                    Err(e) => {
                        let _ = proxy.send_event(UserEvent::PtyError(e.to_string()));
                        return;
                    }
                }
            }
        })
        .expect("spawn pty-reader")
}
