//! CPU, GPU, RAM, and temperature readings for the system widget.

use std::time::Duration;

use sysinfo::{Components, System};
use tracing::info;

use super::WidgetHandle;
use super::model::{SystemStats, WidgetValue};

pub struct SystemSampler {
    sys: System,
    components: Components,
    #[cfg(windows)]
    gpu: Option<gpu::GpuCounter>,
}

impl Default for SystemSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemSampler {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self {
            sys,
            components: Components::new_with_refreshed_list(),
            #[cfg(windows)]
            gpu: gpu::GpuCounter::open(),
        }
    }

    /// Call at least `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` apart; CPU usage is measured between calls.
    pub fn sample(&mut self) -> SystemStats {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.components.refresh(false);
        let mb = |b: u64| (b / (1024 * 1024)).min(u64::from(u32::MAX)) as u32;
        let temp = |want: &[&str]| {
            self.components
                .iter()
                .filter(|c| {
                    let l = c.label().to_ascii_lowercase();
                    want.iter().any(|w| l.contains(w))
                })
                .filter_map(|c| c.temperature())
                .filter(|t| t.is_finite() && *t > 0.0 && *t < 150.0)
                .fold(None, |acc: Option<f32>, t| Some(acc.map_or(t, |a| a.max(t))))
                .map(|t| (t * 10.0).round() as i16)
        };
        SystemStats {
            cpu_pct: Some(self.sys.global_cpu_usage().round().clamp(0.0, 100.0) as u8),
            #[cfg(windows)]
            gpu_pct: self.gpu.as_mut().and_then(|g| g.read()),
            #[cfg(not(windows))]
            gpu_pct: None,
            ram_used_mb: mb(self.sys.used_memory()),
            ram_total_mb: mb(self.sys.total_memory()),
            cpu_temp_dc: temp(&["cpu", "package", "tctl", "core"]),
            gpu_temp_dc: temp(&["gpu"]),
        }
    }
}

/// Samples on its own thread until the hub stops.
pub fn spawn(handle: WidgetHandle, interval: Duration) {
    let spawned = std::thread::Builder::new().name("widget-system".into()).spawn(move || {
        let mut sampler = SystemSampler::new();
        loop {
            std::thread::sleep(interval);
            if !handle.blocking_update(WidgetValue::System(sampler.sample())) {
                break;
            }
        }
    });
    if let Err(e) = spawned {
        info!("system widget disabled: cannot start thread: {e}");
    }
}

#[cfg(windows)]
mod gpu {
    use windows::Win32::System::Performance::{
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW,
        PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };
    use windows::core::w;

    /// Sum of the 3D engine utilisation over all processes (what Task Manager shows as "3D").
    pub struct GpuCounter {
        query: PDH_HQUERY,
        counter: PDH_HCOUNTER,
    }

    // The handles are only used from the sampler's thread; PDH handles are not tied to a thread.
    unsafe impl Send for GpuCounter {}

    impl GpuCounter {
        pub fn open() -> Option<Self> {
            unsafe {
                let mut query = PDH_HQUERY::default();
                if PdhOpenQueryW(None, 0, &mut query) != 0 {
                    return None;
                }
                let mut counter = PDH_HCOUNTER::default();
                if PdhAddEnglishCounterW(
                    query,
                    w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage"),
                    0,
                    &mut counter,
                ) != 0
                {
                    PdhCloseQuery(query);
                    return None;
                }
                PdhCollectQueryData(query);
                Some(Self { query, counter })
            }
        }

        pub fn read(&mut self) -> Option<u8> {
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return None;
                }
                let (mut size, mut count) = (0u32, 0u32);
                if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
                    != PDH_MORE_DATA
                {
                    return None;
                }
                let item = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
                let mut buf: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> =
                    vec![Default::default(); (size as usize).div_ceil(item)];
                if PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    Some(buf.as_mut_ptr()),
                ) != 0
                {
                    return None;
                }
                let total: f64 = buf[..count as usize].iter().map(|i| i.FmtValue.Anonymous.doubleValue).sum();
                Some(total.round().clamp(0.0, 100.0) as u8)
            }
        }
    }

    impl Drop for GpuCounter {
        fn drop(&mut self) {
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads real counters; harmless and quick.
    #[test]
    fn samples_plausible_values() {
        let mut s = SystemSampler::new();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let v = s.sample();
        assert!(v.ram_total_mb > 0 && v.ram_used_mb <= v.ram_total_mb);
        assert!(v.cpu_pct.unwrap() <= 100);
        assert!(v.gpu_pct.is_none_or(|g| g <= 100));
    }
}
