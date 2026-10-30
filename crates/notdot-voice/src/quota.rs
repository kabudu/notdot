//! Cached, provider-specific usage and durable warning history. No credentials or speech.
use crate::usage::{Overage, UsageReport, VoiceErrorKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
fn opaque_id(value: impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&value).expect("notice tuple serializes"))
    )
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ledger {
    pub cache: Option<Cache>,
    seen: Vec<Seen>,
    fault_announcements: Vec<(String, VoiceErrorKind, u64)>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Cache {
    pub provider: String,
    pub report: Option<UsageReport>,
    pub last_checked: u64,
    pub last_success: Option<u64>,
    pub unavailable: Option<String>,
    pub fault: Option<Fault>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Fault {
    #[serde(default)]
    pub operation: String,
    pub kind: VoiceErrorKind,
    pub message: String,
    pub at: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Seen {
    provider: String,
    account: String,
    meter: String,
    reset: u64,
    threshold: u8,
}
#[derive(Clone, Debug)]
pub struct Notice {
    pub id: String,
    pub text: String,
    pub threshold: Option<u8>,
    pub fault: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Status {
    pub provider: String,
    pub report: Option<UsageReport>,
    pub last_checked: Option<u64>,
    pub last_success: Option<u64>,
    pub stale: bool,
    pub availability: String,
    pub message: String,
    pub fault: Option<Fault>,
    pub level: String,
    pub notice_id: Option<String>,
}
impl Ledger {
    pub fn restore_history(&mut self, prior: &Self) {
        self.seen.clone_from(&prior.seen);
        self.fault_announcements
            .clone_from(&prior.fault_announcements);
    }
    pub fn invalidate(&mut self, provider: &str) {
        if self.cache.as_ref().is_some_and(|c| c.provider == provider) {
            self.cache = None;
        }
    }
    pub fn observe(&mut self, provider: &str, report: Result<UsageReport, String>, now: u64) {
        let old = self.cache.take().filter(|c| c.provider == provider);
        let (report, success, unavailable) = match report {
            Ok(report) => (Some(report), Some(now), None),
            Err(reason) => (
                old.as_ref().and_then(|c| c.report.clone()),
                old.as_ref().and_then(|c| c.last_success),
                Some(reason),
            ),
        };
        self.cache = Some(Cache {
            provider: provider.into(),
            report,
            last_checked: now,
            last_success: success,
            unavailable,
            fault: old.and_then(|c| c.fault),
        });
    }
    pub fn fault(&mut self, provider: &str, kind: VoiceErrorKind, message: &str, now: u64) {
        if !self.cache.as_ref().is_some_and(|c| c.provider == provider) {
            self.observe(
                provider,
                Err("Usage unavailable: not checked yet".into()),
                now,
            );
            self.cache.as_mut().expect("cache initialized").last_checked = 0;
        }
        self.cache.as_mut().expect("cache initialized").fault = Some(Fault {
            operation: String::new(),
            kind,
            message: message.into(),
            at: now,
        });
    }
    pub fn recovered(&mut self, provider: &str) {
        if let Some(c) = self.cache.as_mut().filter(|c| c.provider == provider) {
            c.fault = None;
        }
    }
    fn threshold_notice(&self, provider: &str, now: u64) -> Option<Notice> {
        let c = self.cache.as_ref().filter(|c| {
            c.provider == provider
                && c.unavailable.is_none()
                && c.last_success
                    .is_some_and(|t| t <= now && now.saturating_sub(t) < 3600)
        })?;
        let report = c.report.as_ref()?;
        for meter in &report.meters {
            let Some(threshold) = meter.threshold() else {
                continue;
            };
            let Some(reset) = meter.reset_unix.filter(|r| *r > now) else {
                continue;
            };
            let seen = self
                .seen
                .iter()
                .find(|s| {
                    s.provider == provider
                        && s.account == report.account
                        && s.meter == meter.id
                        && s.reset == reset
                })
                .map_or(0, |s| s.threshold);
            if threshold <= seen {
                continue;
            }
            let billing = if threshold == 100 {
                if meter.overage == Overage::Enabled {
                    "The provider reports usage based billing is enabled. Continued use may incur charges."
                } else {
                    "Other API or prepaid balances may differ. Check your provider account."
                }
            } else {
                "Separate API and transcription balances may differ."
            };
            return Some(Notice {
                id: opaque_id((provider, &report.account, &meter.id, reset, threshold)),
                text: format!(
                    "Your {provider} subscription allowance has reached {threshold} percent. {billing}"
                ),
                threshold: Some(threshold),
                fault: false,
            });
        }
        None
    }
    pub fn notice(&self, provider: &str, now: u64) -> Option<Notice> {
        let c = self.cache.as_ref().filter(|c| c.provider == provider)?;
        if let Some(fault) = c.fault.as_ref().filter(|f| now.saturating_sub(f.at) < 3600) {
            let cooldown = if fault.kind == VoiceErrorKind::Quota {
                1800
            } else {
                300
            };
            let recent = self.fault_announcements.iter().any(|(p, k, at)| {
                p == provider && *k == fault.kind && now.saturating_sub(*at) < cooldown
            });
            if !recent {
                return Some(Notice {
                    id: opaque_id(("fault", provider, fault.kind, fault.at)),
                    text: format!("{provider}. {}", fault.message),
                    threshold: None,
                    fault: true,
                });
            }
        }
        self.threshold_notice(provider, now)
    }
    /// Mark before local speech to avoid duplicate announcements across a process restart.
    /// A crash during speech can therefore lose that one announcement, never install or spend.
    pub fn mark(&mut self, provider: &str, id: &str, now: u64) -> Option<String> {
        let notice = self.notice(provider, now).filter(|n| n.id == id)?;
        if notice.fault {
            let kind = self.cache.as_ref()?.fault.as_ref()?.kind;
            self.fault_announcements
                .retain(|(p, k, _)| p != provider || *k != kind);
            if self.fault_announcements.len() >= 32 {
                self.fault_announcements.remove(0);
            }
            self.fault_announcements.push((provider.into(), kind, now));
            if kind == VoiceErrorKind::Quota
                && let Some(threshold) = self
                    .threshold_notice(provider, now)
                    .filter(|n| n.threshold == Some(100))
            {
                self.mark_threshold(provider, &threshold);
            }
        } else {
            self.mark_threshold(provider, &notice);
        }
        Some(notice.text)
    }
    fn mark_threshold(&mut self, provider: &str, notice: &Notice) {
        let Some(report) = self.cache.as_ref().and_then(|c| c.report.as_ref()) else {
            return;
        };
        let Some(meter) = report.meters.iter().find(|m| {
            m.reset_unix.is_some()
                && opaque_id((
                    provider,
                    &report.account,
                    &m.id,
                    m.reset_unix.unwrap_or(0),
                    notice.threshold.unwrap_or(0),
                )) == notice.id
        }) else {
            return;
        };
        let entry = Seen {
            provider: provider.into(),
            account: report.account.clone(),
            meter: meter.id.clone(),
            reset: meter.reset_unix.unwrap_or(0),
            threshold: notice.threshold.unwrap_or(0),
        };
        self.seen.retain(|s| {
            !(s.provider == entry.provider
                && s.account == entry.account
                && s.meter == entry.meter
                && s.reset == entry.reset)
        });
        if self.seen.len() >= 64 {
            self.seen.remove(0);
        }
        self.seen.push(entry);
    }
    pub fn status(&self, provider: &str, now: u64) -> Status {
        let cache = self.cache.as_ref().filter(|c| c.provider == provider);
        let stale = cache
            .and_then(|c| c.last_success)
            .is_none_or(|t| now.saturating_sub(t) >= 3600 || t > now)
            || cache.and_then(|c| c.report.as_ref()).is_some_and(|r| {
                r.meters
                    .iter()
                    .any(|m| m.reset_unix.is_some_and(|reset| reset <= now))
            });
        let available = cache.is_some_and(|c| c.unavailable.is_none() && c.report.is_some());
        let mut report = cache.and_then(|c| c.report.clone());
        // Account identities are needed for private warning deduplication, not the UI.
        if let Some(r) = &mut report {
            r.account.clear();
        }
        let fault = cache.and_then(|c| c.fault.clone());
        let level = if fault
            .as_ref()
            .is_some_and(|f| f.kind == VoiceErrorKind::Quota && now.saturating_sub(f.at) < 3600)
        {
            "exhausted"
        } else if available
            && !stale
            && report.as_ref().is_some_and(|r| {
                r.meters
                    .iter()
                    .any(|m| m.threshold().is_some() && m.reset_unix.is_none_or(|t| t > now))
            })
        {
            "warning"
        } else {
            "normal"
        };
        Status {
            provider: provider.into(),
            report,
            last_checked: cache.map(|c| c.last_checked).filter(|t| *t > 0),
            last_success: cache.and_then(|c| c.last_success),
            stale,
            availability: if available {
                "available"
            } else {
                "unavailable"
            }
            .into(),
            message: cache
                .and_then(|c| c.unavailable.clone())
                .unwrap_or_else(|| {
                    if available {
                        "Provider-reported subscription usage".into()
                    } else {
                        "Usage unavailable: not checked yet".into()
                    }
                }),
            fault,
            level: level.into(),
            notice_id: self.notice(provider, now).map(|n| n.id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::tests::report;
    const PROVIDER: &str = "elevenlabs";
    #[test]
    fn thresholds_survive_restart_and_reset_for_a_new_cycle_or_account() {
        let mut ledger = Ledger::default();
        ledger.observe(PROVIDER, Ok(report(799, Some(10000))), 100);
        assert!(ledger.notice(PROVIDER, 100).is_none());
        for (used, threshold) in [(800, 80), (950, 90), (1000, 100)] {
            ledger.observe(PROVIDER, Ok(report(used, Some(10000))), 100);
            let notice = ledger.notice(PROVIDER, 100).unwrap();
            assert_eq!(notice.threshold, Some(threshold));
            assert!(!notice.id.contains("account"));
            ledger.mark(PROVIDER, &notice.id, 100).unwrap();
            ledger = serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
            assert!(ledger.notice(PROVIDER, 101).is_none());
        }
        ledger.invalidate(PROVIDER);
        ledger.observe(PROVIDER, Ok(report(1000, Some(10000))), 200);
        assert!(ledger.notice(PROVIDER, 200).is_none());
        ledger.observe(PROVIDER, Ok(report(800, Some(20000))), 200);
        assert_eq!(ledger.notice(PROVIDER, 200).unwrap().threshold, Some(80));
        let mut other = report(800, Some(10000));
        other.account = "another-account".into();
        ledger.observe(PROVIDER, Ok(other), 200);
        assert_eq!(ledger.notice(PROVIDER, 200).unwrap().threshold, Some(80));
    }
    #[test]
    fn unknown_stale_expired_or_permission_denied_usage_never_announces_fake_balance() {
        let mut ledger = Ledger::default();
        assert_eq!(ledger.status(PROVIDER, 100).availability, "unavailable");
        ledger.observe(PROVIDER, Ok(report(900, None)), 100);
        assert!(ledger.notice(PROVIDER, 100).is_none());
        ledger.observe(PROVIDER, Ok(report(900, Some(200))), 100);
        assert!(ledger.notice(PROVIDER, 201).is_none());
        assert!(ledger.status(PROVIDER, 201).stale);
        ledger.observe(PROVIDER, Ok(report(900, Some(10000))), 100);
        assert!(ledger.notice(PROVIDER, 4000).is_none());
        ledger.observe(
            PROVIDER,
            Err("Usage unavailable: permission denied".into()),
            110,
        );
        let status = ledger.status(PROVIDER, 111);
        assert_eq!(status.availability, "unavailable");
        assert!(status.report.is_some());
        assert!(ledger.notice(PROVIDER, 111).is_none());
        assert_eq!(status.level, "normal");
        assert!(ledger.status("another-backend", 111).report.is_none());
    }
    #[test]
    fn fault_alerts_are_local_messages_and_do_not_confuse_rate_limits_with_credits() {
        let mut ledger = Ledger::default();
        ledger.observe(PROVIDER, Ok(report(1000, Some(10000))), 100);
        ledger.fault(PROVIDER, VoiceErrorKind::Quota, "Insufficient credits", 100);
        let notice = ledger.notice(PROVIDER, 100).unwrap();
        assert!(notice.fault);
        assert_eq!(ledger.status(PROVIDER, 100).level, "exhausted");
        ledger.mark(PROVIDER, &notice.id, 100).unwrap();
        assert!(ledger.notice(PROVIDER, 101).is_none());
        ledger.recovered(PROVIDER);
        ledger.observe(PROVIDER, Ok(report(100, Some(10000))), 102);
        ledger.fault(
            PROVIDER,
            VoiceErrorKind::RateLimit,
            "Try again shortly",
            103,
        );
        assert_eq!(ledger.status(PROVIDER, 103).level, "normal");
        let notice = ledger.notice(PROVIDER, 103).unwrap();
        assert!(notice.text.contains("Try again"));
        ledger.mark(PROVIDER, &notice.id, 103);
        assert!(ledger.notice(PROVIDER, 104).is_none());
    }
    #[test]
    fn threshold_jumps_coalesce_and_other_meters_still_get_checked() {
        let mut ledger = Ledger::default();
        let mut r = report(990, Some(10000));
        let mut quiet = r.meters[0].clone();
        quiet.used = 0;
        quiet.id = "quiet".into();
        r.meters.insert(0, quiet);
        ledger.observe(PROVIDER, Ok(r), 100);
        let n = ledger.notice(PROVIDER, 100).unwrap();
        assert_eq!(n.threshold, Some(90));
        ledger.mark(PROVIDER, &n.id, 100).unwrap();
        assert!(ledger.notice(PROVIDER, 101).is_none());
    }
}
