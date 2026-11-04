//! Local protocol helper and authenticated read-only chat lookup smoke test.
use std::path::Path;
fn main() {
    let runtime = std::env::args_os()
        .skip(1)
        .find(|a| a != "--codex-chat-tools")
        .expect("usage: chat_tools CODEX_RUNTIME");
    if let Err(error) = notdot_core::chat_tools::serve(Path::new(&runtime)) {
        eprintln!("Chat lookup failed: {error}");
        std::process::exit(1);
    }
}
