//! Model-free child for pipe cancellation/reap tests, never a recognition helper.
//! Tauri and the packaged Windows helper build select their binaries explicitly;
//! this distinct test fixture is not selected or shipped by either build.
#![forbid(unsafe_code)]
use notdot_wake_runtime::protocol;
use std::io::Write;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "--model" {
        return Err("invalid fixture arguments".into());
    }
    // The path is deliberately unused: model validity belongs to real-helper
    // tests. This process tests the real Client/pipe/OS lifecycle alone.
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    protocol::read_hello(&mut input)?;
    output.write_all(protocol::READY)?;
    output.flush()?;
    if protocol::read_request(&mut input)?.is_some() {
        // No reply races cancellation or a zero-budget timeout. The finite
        // delay also bounds this fixture if its parent fails to terminate it.
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    Ok(())
}
fn main() {
    if run().is_err() {
        std::process::exit(1);
    }
}
