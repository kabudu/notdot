//! One private helper, one in-flight job, absolute caller-selected package paths.
//! The app must apply package::select (including its CPU guard) before start.
use crate::protocol;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{interval, timeout},
};
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Cancelled,
    Unavailable,
}
// A lost OS wait must not become an apparent successful release merely because
// its Client was dropped during cancellation, failed startup, or generic fallback.
struct ReapRegistry(AtomicUsize);
impl ReapRegistry {
    fn spawned(&self) -> ReapTicket<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        ReapTicket {
            registry: self,
            confirmed: false,
        }
    }
    fn clear(&self) -> bool {
        self.0.load(Ordering::SeqCst) == 0
    }
}
struct ReapTicket<'a> {
    registry: &'a ReapRegistry,
    confirmed: bool,
}
impl ReapTicket<'_> {
    fn confirm(&mut self) {
        if !self.confirmed {
            self.registry.0.fetch_sub(1, Ordering::SeqCst);
            self.confirmed = true;
        }
    }
}
static CHILDREN: ReapRegistry = ReapRegistry(AtomicUsize::new(0));
/// False remains sticky after an unconfirmed child is dropped: restart is needed
/// before replacing its executable, even if a later decoder has no child handle.
pub fn all_helpers_reaped() -> bool {
    CHILDREN.clear()
}
pub struct Client {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    next: u64,
    reap: ReapTicket<'static>,
}
impl Client {
    pub async fn start(helper: &Path, model: &Path, phrase: &str) -> Result<Self, Failure> {
        Self::start_cancellable(helper, model, phrase, None).await
    }
    pub async fn start_cancellable(
        helper: &Path,
        model: &Path,
        phrase: &str,
        cancel: Option<(Arc<AtomicU64>, u64)>,
    ) -> Result<Self, Failure> {
        if cancel
            .as_ref()
            .is_some_and(|(epoch, expected)| epoch.load(Ordering::Relaxed) != *expected)
        {
            return Err(Failure::Cancelled);
        }
        if !helper.is_absolute() || !model.is_absolute() {
            return Err(Failure::Unavailable);
        }
        let mut command = Command::new(helper);
        command
            .current_dir(helper.parent().ok_or(Failure::Unavailable)?)
            .arg("--model")
            .arg(model)
            .env_clear()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(feature = "synthetic-diagnostics")]
        if crate::engine::synthetic_diagnostics_enabled() {
            command
                .env("NOTDOT_SYNTHETIC_WAKE_DIAGNOSTICS", "synthetic-ci")
                .stderr(std::process::Stdio::inherit());
        }
        #[cfg(windows)]
        {
            command.creation_flags(0x08000000);
            for key in ["SystemRoot", "WINDIR"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
        }
        let mut child = command.spawn().map_err(|_| Failure::Unavailable)?;
        let reap = CHILDREN.spawned();
        let input = child.stdin.take().ok_or(Failure::Unavailable)?;
        let output = child.stdout.take().ok_or(Failure::Unavailable)?;
        let mut client = Self {
            child,
            input,
            output,
            next: 1,
            reap,
        };
        let mut cancellation = interval(Duration::from_millis(20));
        let ready = tokio::select! {
        ready=timeout(Duration::from_secs(3), async {
            let mut hello = Vec::new();
            protocol::write_hello(&mut hello, phrase)?;
            client.input.write_all(&hello).await?;
            client.input.flush().await?;
            let mut ready = [0; 8];
            client.output.read_exact(&mut ready).await?;
            if &ready != protocol::READY {
                return Err(std::io::Error::other("Invalid wake helper greeting"));
            }
            Ok::<_, std::io::Error>(())
        })=>match ready {Ok(Ok(()))=>Ok(()),_=>Err(Failure::Unavailable)},
        _=async {loop {cancellation.tick().await;if cancel.as_ref().is_some_and(|(epoch,expected)|epoch.load(Ordering::Relaxed)!=*expected) {break;}}}=>Err(Failure::Cancelled)
        };
        if let Err(error) = ready {
            client.shutdown().await;
            return Err(error);
        }
        Ok(client)
    }
    pub async fn recognize(
        &mut self,
        samples: &[f32],
        epoch: Arc<AtomicU64>,
        expected: u64,
        budget: Duration,
    ) -> Result<bool, Failure> {
        if epoch.load(Ordering::Relaxed) != expected {
            self.shutdown().await;
            return Err(Failure::Cancelled);
        }
        let id = self.next;
        let Some(next) = self.next.checked_add(1) else {
            self.shutdown().await;
            return Err(Failure::Unavailable);
        };
        self.next = next;
        let packet = match protocol::encode_request(id, samples) {
            Ok(packet) => packet,
            Err(_) => {
                self.shutdown().await;
                return Err(Failure::Unavailable);
            }
        };
        let mut cancellation = interval(Duration::from_millis(20));
        let result = tokio::select! {
            result=timeout(budget,async {
                self.input.write_all(&packet).await?;self.input.flush().await?;
                let mut response=[0;9];self.output.read_exact(&mut response).await?;
                protocol::parse_response(&response,id)
            })=>match result {Ok(Ok(heard))=>Ok(heard),_=>Err(Failure::Unavailable)},
            _=async {loop {cancellation.tick().await;if epoch.load(Ordering::Relaxed)!=expected {break;}}}=>Err(Failure::Cancelled),
        };
        let result = if epoch.load(Ordering::Relaxed) != expected {
            Err(Failure::Cancelled)
        } else {
            result
        };
        if result.is_err() {
            self.shutdown().await;
        }
        result
    }
    /// True only after the OS confirms that the child has exited and been reaped.
    pub async fn shutdown(&mut self) -> bool {
        let _ = self.child.start_kill();
        // kill_on_drop remains armed even if an OS wait fails to complete.
        let confirmed = matches!(
            timeout(Duration::from_millis(500), self.child.wait()).await,
            Ok(Ok(_))
        );
        if confirmed {
            self.reap.confirm();
        }
        confirmed
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unconfirmed_cancel_cleanup_stays_blocked_after_client_and_decoder_are_gone() {
        let registry = ReapRegistry(AtomicUsize::new(0));
        {
            let _cancelled_child = registry.spawned();
        }
        assert!(
            !registry.clear(),
            "dropping a cancelled child is not a confirmed OS reap"
        );
        {
            let mut later_decoder = registry.spawned();
            later_decoder.confirm();
            later_decoder.confirm();
        }
        assert!(
            !registry.clear(),
            "a later successful decoder cannot clear an old failure"
        );
        let healthy = ReapRegistry(AtomicUsize::new(0));
        let mut child = healthy.spawned();
        child.confirm();
        child.confirm();
        assert!(healthy.clear());
    }
}
