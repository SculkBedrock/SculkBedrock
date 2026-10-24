//! 控制台驱动：本进程资源采样（由 `driver.rs` 拆出；在 `crate::driver` 重新导出）。
//!
//! Console driver: own-process resource sampling (split from `driver.rs`; re-exported at `crate::driver`).

use super::*;

// ================= Own-process resource sampling =================

/// 本进程 CPU/内存采样器（由控制台线程本地持有，不涉及宿主）。
///
/// - 只刷新本进程（`ProcessesToUpdate::Some`），每秒采样一次；
///   CPU % 需两次采样后才有效，之前为占位值；
/// - CPU % 归一化为整机 0-100%（除以逻辑 CPU 数并截断）；
/// - macOS 内存使用 `phys_footprint`（与活动监视器一致），取不到时回退到 sysinfo RSS；
///   其他平台使用 RSS。
///
/// Own-process CPU/memory sampler (owned locally by the console thread; no host involvement).
///
/// - Refreshes only this process (`ProcessesToUpdate::Some`);
///   samples every second, CPU % valid only after two samples (placeholder before);
/// - CPU % is normalized to whole-machine 0-100% (divided by logical CPU count and clamped);
/// - On macOS memory uses `phys_footprint` (matches Activity Monitor);
///   falls back to sysinfo RSS when unavailable; RSS elsewhere.
pub(super) struct ProcSampler {
    pub(super) sys: sysinfo::System,
    pub(super) pid: sysinfo::Pid,
    pub(super) cpu_pct: f32,
    pub(super) mem_mb: f64,
    pub(super) samples: u32,
    pub(super) last: std::time::Instant,
    /// Logical CPU count (for normalization, at least 1).
    pub(super) num_cpus: f32,
}

/// Own-process `phys_footprint` on macOS (bytes), matching the Activity Monitor memory column.
/// Returns `None` when unavailable so the caller falls back to RSS.
#[cfg(target_os = "macos")]
pub(super) fn macos_footprint_bytes() -> Option<u64> {
    use std::mem::MaybeUninit;
    unsafe {
        let mut info = MaybeUninit::<libc::rusage_info_v2>::uninit();
        let ret = libc::proc_pid_rusage(
            std::process::id() as libc::c_int,
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr() as *mut _,
        );
        if ret < 0 {
            return None;
        }
        let info = info.assume_init();
        let fp = info.ri_phys_footprint;
        if fp > 0 {
            Some(fp)
        } else {
            None
        }
    }
}

/// Normalize raw sysinfo CPU (single-core 100%, multicore can exceed 100%) to whole-machine 0-100%.
pub(super) fn normalize_cpu_pct(raw: f32, num_cpus: f32) -> f32 {
    let n = num_cpus.max(1.0);
    (raw / n).clamp(0.0, 100.0)
}

impl ProcSampler {
    pub(super) fn new() -> Self {
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get() as f32)
            .unwrap_or(1.0)
            .max(1.0);
        Self {
            sys: sysinfo::System::new(),
            pid: sysinfo::Pid::from_u32(std::process::id()),
            cpu_pct: 0.0,
            mem_mb: 0.0,
            samples: 0,
            last: std::time::Instant::now() - Duration::from_secs(2),
            num_cpus,
        }
    }

    /// Return the snapshot plus whether a new sample arrived. Callers repaint only on new samples.
    pub(super) fn poll(&mut self) -> (ConsoleStats, bool) {
        if self.last.elapsed() < Duration::from_secs(1) {
            return (
                ConsoleStats {
                    cpu_pct: self.cpu_pct,
                    mem_mb: self.mem_mb,
                    has_data: self.samples >= 2,
                },
                false,
            );
        }
        self.last = std::time::Instant::now();
        let pid = self.pid;
        self.sys
            .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        if let Some(process) = self.sys.process(pid) {
            self.samples = self.samples.saturating_add(1);
            // CPU % measures the delta between refreshes: valid from the second sample on.
            // Raw sysinfo CPU is single-core 100% (multicore can exceed 100%); normalize to 0-100%.
            if self.samples >= 2 {
                self.cpu_pct = normalize_cpu_pct(process.cpu_usage(), self.num_cpus);
            }
            #[cfg(target_os = "macos")]
            let mem_bytes = macos_footprint_bytes().unwrap_or_else(|| process.memory());
            #[cfg(not(target_os = "macos"))]
            let mem_bytes = process.memory();
            self.mem_mb = mem_bytes as f64 / 1_048_576.0;
        }
        (
            ConsoleStats {
                cpu_pct: self.cpu_pct,
                mem_mb: self.mem_mb,
                has_data: self.samples >= 2,
            },
            true,
        )
    }
}
