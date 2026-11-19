//! Backup wake decoder state, available whether or not it is built in.

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BackupState {
    /// Not built into this library, or turned off for this engine.
    Off,
    Loading,
    Ready,
    /// The model could not load; the keyword spotter still works.
    Unavailable,
}

/// Counters for measuring the backup on a device. No audio or text.
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct BackupStats {
    pub decodes: u64,
    pub wakes: u64,
    pub failures: u64,
    pub decode_ms_total: u64,
    pub decode_ms_max: u64,
    /// Utterances dropped because decoding could not keep up with speech.
    pub dropped: u64,
}
