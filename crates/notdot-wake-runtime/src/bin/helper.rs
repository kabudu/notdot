#![forbid(unsafe_code)]
use notdot_wake_runtime::{
    engine::{self, DecodeLimits},
    protocol,
};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};
use whisper_rs::{WhisperContext, WhisperContextParameters};
fn run() -> Result<(), ()> {
    // Parent checks before process launch: static accelerated code may initialise
    // before main. This repeated check is defence in depth, not the primary gate.
    #[cfg(feature = "accelerated")]
    if !notdot_wake_runtime::package::capable_cpu() {
        return Err(());
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "--model" {
        return Err(());
    }
    let model = PathBuf::from(&args[1]);
    if !model.is_absolute() {
        return Err(());
    }
    let metadata = std::fs::metadata(&model).map_err(|_| ())?;
    if metadata.len() == 0 || metadata.len() > 64 * 1024 * 1024 {
        return Err(());
    }
    let manifest = include_str!("../../../../src-tauri/resources/wake/SHA256SUMS");
    let digest = manifest
        .lines()
        .find_map(|line| {
            line.split_once("  ")
                .filter(|(_, name)| *name == "ggml-tiny.en-q5_1.bin")
                .map(|(sha, _)| sha)
        })
        .ok_or(())?;
    let mut hash = Sha256::new();
    let mut file = std::fs::File::open(&model).map_err(|_| ())?;
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|_| ())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if format!("{:x}", hash.finalize()) != digest {
        return Err(());
    }
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let phrase = protocol::read_hello(&mut input).map_err(|_| ())?;
    whisper_rs::install_logging_hooks();
    let mut params = WhisperContextParameters::default();
    params.use_gpu(false).flash_attn(false);
    let context = WhisperContext::new_with_params(model, params).map_err(|_| ())?;
    output.write_all(protocol::READY).map_err(|_| ())?;
    output.flush().map_err(|_| ())?;
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(4)) as i32;
    let mut last = 0;
    while let Some((id, samples)) = protocol::read_request(&mut input).map_err(|_| ())? {
        if id <= last {
            return Err(());
        }
        last = id;
        let result = engine::recognize(
            &context,
            &phrase,
            &samples,
            16000,
            Arc::new(AtomicU64::new(0)),
            0,
            DecodeLimits {
                budget: Duration::from_secs(3),
                threads,
            },
        )
        .map_err(|_| ());
        output
            .write_all(&protocol::response(id, result))
            .map_err(|_| ())?;
        output.flush().map_err(|_| ())?;
    }
    Ok(())
}
fn main() {
    if run().is_err() {
        std::process::exit(1);
    }
}
