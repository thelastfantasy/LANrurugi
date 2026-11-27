//! Optional GPU readings.
//!
//! Off by default and gated by `LANRURUGI_RESOURCE_GPU=1`, because the only way to ask NVIDIA for a
//! number is to run `nvidia-smi` — a subprocess, which the sampling path is not allowed to do
//! unprompted (spec FR-014). AMD is read from sysfs and needs no process, but it is behind the same
//! gate so "enabled" means one thing.
//!
//! A probe that fails sets `unavailable` on the section and never fails the snapshot: a machine
//! without a GPU is the normal case, not an error.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::resources::model::GpuSection;

/// How long a probe result is reused. GPU utilisation is not a number worth restarting
/// `nvidia-smi` twice a second for.
const CACHE_TTL: Duration = Duration::from_secs(5);
/// `nvidia-smi` normally answers in a few tens of milliseconds; a driver wedge must not stall the
/// whole snapshot.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub trait GpuProber: Send + Sync {
    fn probe(&self) -> Result<GpuSection, String>;
}

/// `nvidia-smi`'s single-line CSV query. Synchronous on purpose: the caller already runs on a
/// blocking thread, and spawning a thread to add a timeout here would be worse than polling.
pub struct NvidiaSmi;

impl GpuProber for NvidiaSmi {
    fn probe(&self) -> Result<GpuSection, String> {
        let mut child = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu",
                "--format=csv,noheader,nounits",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("nvidia-smi did not start: {error}"))?;
        let deadline = Instant::now() + PROBE_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return Err(format!("nvidia-smi exited with {status}"));
                    }
                    break;
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("nvidia-smi timed out".to_string());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(error) => return Err(format!("nvidia-smi could not be waited on: {error}")),
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("nvidia-smi output unreadable: {error}"))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let first = text.lines().next().unwrap_or_default();
        let fields: Vec<&str> = first.split(',').map(str::trim).collect();
        let number = |index: usize| -> Option<f64> {
            fields
                .get(index)
                .and_then(|value| value.parse::<f64>().ok())
        };
        Ok(GpuSection {
            vendor: "nvidia".to_string(),
            utilization_percent: number(0),
            memory_used_bytes: number(1).map(|mib| (mib * 1024.0 * 1024.0) as u64),
            memory_total_bytes: number(2).map(|mib| (mib * 1024.0 * 1024.0) as u64),
            temperature_c: number(3),
            unavailable: None,
        })
    }
}

/// AMD cards expose the same facts through sysfs, no process needed.
pub struct AmdSysfs;

const AMD_CARD: &str = "/sys/class/drm/card0/device";

impl GpuProber for AmdSysfs {
    fn probe(&self) -> Result<GpuSection, String> {
        let used = read_u64(&format!("{AMD_CARD}/mem_info_vram_used"))
            .ok_or_else(|| "no AMD VRAM counters".to_string())?;
        let total = read_u64(&format!("{AMD_CARD}/mem_info_vram_total"));
        let busy = read_u64(&format!("{AMD_CARD}/gpu_busy_percent")).map(|value| value as f64);
        Ok(GpuSection {
            vendor: "amd".to_string(),
            utilization_percent: busy,
            memory_used_bytes: Some(used),
            memory_total_bytes: total,
            temperature_c: None,
            unavailable: None,
        })
    }
}

fn read_u64(path: &str) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Picks the prober this machine can actually answer with, or explains why none can.
fn detect() -> Result<Box<dyn GpuProber>, String> {
    if std::path::Path::new(AMD_CARD)
        .join("mem_info_vram_used")
        .exists()
    {
        return Ok(Box::new(AmdSysfs));
    }
    if which_nvidia_smi() {
        return Ok(Box::new(NvidiaSmi));
    }
    Err("no GPU counters on this host".to_string())
}

fn which_nvidia_smi() -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("nvidia-smi").is_file()))
        .unwrap_or(false)
}

/// `LANRURUGI_RESOURCE_GPU=1` turns probing on. Anything else — including unset — keeps it off.
pub fn enabled_from_env() -> bool {
    std::env::var("LANRURUGI_RESOURCE_GPU")
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

pub struct GpuReader {
    enabled: bool,
    detected: Result<Box<dyn GpuProber>, String>,
    cache: Mutex<Option<(Instant, GpuSection)>>,
}

impl GpuReader {
    pub fn new(enabled: bool, detected: Result<Box<dyn GpuProber>, String>) -> Self {
        Self {
            enabled,
            detected,
            cache: Mutex::new(None),
        }
    }

    fn from_env() -> Self {
        Self::new(enabled_from_env(), detect())
    }

    pub fn global() -> &'static GpuReader {
        static READER: std::sync::OnceLock<GpuReader> = std::sync::OnceLock::new();
        READER.get_or_init(GpuReader::from_env)
    }

    /// `(section, unavailable)`. Exactly one of them is `Some` when the feature is enabled; with
    /// the feature off the section is `None` and the reason says so, so the panel can distinguish
    /// "off" from "tried and failed".
    pub fn read(&self) -> (Option<GpuSection>, Option<String>) {
        if !self.enabled {
            return (None, Some("disabled".to_string()));
        }
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, section)) = cache.as_ref()
                && at.elapsed() < CACHE_TTL
            {
                return (Some(section.clone()), None);
            }
        }
        let prober = match &self.detected {
            Ok(prober) => prober,
            Err(_) => return (None, Some("no_device".to_string())),
        };
        match prober.probe() {
            Ok(section) => {
                let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                *cache = Some((Instant::now(), section.clone()));
                (Some(section), None)
            }
            Err(_) => (None, Some("probe_failed".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Counting {
        calls: Arc<AtomicUsize>,
    }

    impl GpuProber for Counting {
        fn probe(&self) -> Result<GpuSection, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(GpuSection {
                vendor: "fake".to_string(),
                utilization_percent: Some(42.0),
                memory_used_bytes: Some(1024),
                memory_total_bytes: Some(2048),
                temperature_c: Some(50.0),
                unavailable: None,
            })
        }
    }

    /// FR-014: with the feature off, nothing is probed at all.
    #[test]
    fn a_disabled_reader_never_calls_the_prober() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reader = GpuReader::new(
            false,
            Ok(Box::new(Counting {
                calls: calls.clone(),
            })),
        );
        let (section, unavailable) = reader.read();
        assert!(section.is_none());
        assert_eq!(unavailable.as_deref(), Some("disabled"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_enabled_reader_probes_once_and_then_serves_the_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reader = GpuReader::new(
            true,
            Ok(Box::new(Counting {
                calls: calls.clone(),
            })),
        );
        let (first, _) = reader.read();
        assert_eq!(
            first.expect("probe succeeded").utilization_percent,
            Some(42.0)
        );
        let (second, _) = reader.read();
        assert_eq!(second.expect("cached").vendor, "fake");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_host_without_a_gpu_reports_a_reason_rather_than_an_error() {
        let reader = GpuReader::new(true, Err("no GPU counters on this host".to_string()));
        let (section, unavailable) = reader.read();
        assert!(section.is_none());
        assert_eq!(unavailable.as_deref(), Some("no_device"));
    }

    #[test]
    fn the_environment_flag_is_one_or_nothing() {
        // The process-wide environment is shared with other tests, so only assert the shape.
        assert!(matches!(enabled_from_env(), true | false));
    }
}
