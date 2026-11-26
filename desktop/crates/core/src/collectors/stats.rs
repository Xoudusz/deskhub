//! PC stats: CPU, RAM, GPU (any vendor, via PDH), disk, network. Port of companion/deskhub/stats.py.
//!
//! The JSON keys and units match stats.py: percent for cpu/gpu/disk, GiB for RAM and VRAM,
//! KiB/s for network. GPU fields are omitted when no GPU reading exists.

use std::time::Instant;

use serde_json::{json, Value};
use sysinfo::{Disks, Networks, System};

use super::Ctx;

const GIB: f64 = 1_073_741_824.0;
const KIB: f64 = 1024.0;

pub async fn run(ctx: Ctx) {
    let out = ctx.out;
    let mut sys = System::new_all();
    sys.refresh_cpu_usage();
    let cpu_name = cpu_name_clean(sys.cpus().first().map(|c| c.brand()).unwrap_or(""));
    let mut nets = Networks::new_with_refreshed_list();
    let mut disks = Disks::new_with_refreshed_list();
    let drive = system_drive();
    let mut last = Instant::now();
    let gpu = gpu::start();
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));

    loop {
        tick.tick().await;
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        nets.refresh(true);
        disks.refresh(true);
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64();
        last = now;

        // sysinfo reports bytes since the previous refresh, so these are per-tick deltas.
        let (rx, tx) = nets.list().values().fold((0u64, 0u64), |(r, t), n| {
            (r + n.received(), t + n.transmitted())
        });
        let disk_pct = disks
            .list()
            .iter()
            .find(|d| {
                d.mount_point()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&drive)
            })
            .map(|d| disk_percent(d.total_space(), d.available_space()))
            .unwrap_or(0.0);
        let g = gpu::snapshot(&gpu);

        let sample = StatsSample {
            cpu: sys.global_cpu_usage() as f64,
            ram_used_bytes: sys.used_memory(),
            ram_total_bytes: sys.total_memory(),
            net_down_kbs: net_rate_kbs(rx, dt),
            net_up_kbs: net_rate_kbs(tx, dt),
            disk_pct,
            cpu_name: cpu_name.clone(),
            gpu_name: g.name,
            gpu: g.fields,
        };
        out.publish("stats", build_stats_json(&sample));
    }
}

/// One sample, already in stats.py units (before rounding).
#[derive(Clone, Debug, PartialEq)]
pub struct StatsSample {
    pub cpu: f64,
    pub ram_used_bytes: u64,
    pub ram_total_bytes: u64,
    pub net_down_kbs: f64,
    pub net_up_kbs: f64,
    pub disk_pct: f64,
    pub cpu_name: String,
    pub gpu_name: String,
    pub gpu: Option<gpu::GpuFields>,
}

/// Builds the `stats` message. Same keys, units and rounding as stats.py.
pub fn build_stats_json(s: &StatsSample) -> Value {
    let mut msg = json!({
        "type": "stats",
        "cpu": round_to(clamp_pct(s.cpu), 1),
        "ram_used": round_to(s.ram_used_bytes as f64 / GIB, 2),
        "ram_total": round_to(s.ram_total_bytes as f64 / GIB, 1),
        "net_down": round_to(s.net_down_kbs, 1),
        "net_up": round_to(s.net_up_kbs, 1),
        "disk": round_to(s.disk_pct, 1),
        "cpu_name": s.cpu_name,
        "gpu_name": s.gpu_name,
    });
    if let (Some(g), Value::Object(map)) = (&s.gpu, &mut msg) {
        map.insert("gpu".into(), json!(round_to(clamp_pct(g.util), 1)));
        if let Some(used) = g.vram_used_bytes {
            map.insert("vram_used".into(), json!(round_to(used as f64 / GIB, 2)));
        }
        if let Some(total) = g.vram_total_bytes {
            map.insert("vram_total".into(), json!(round_to(total as f64 / GIB, 1)));
        }
    }
    msg
}

/// Same as stats.py `_cpu_name`: drop brand noise, keep the part before '@', fold to ASCII.
pub fn cpu_name_clean(raw: &str) -> String {
    let mut s = raw.to_string();
    for junk in ["(R)", "(TM)", "CPU", "Processor"] {
        s = s.replace(junk, "");
    }
    ascii_fold(s.split('@').next().unwrap_or(""), 40)
}

/// Text the board can draw, at most `limit` bytes (crate::text::board_text): Latin-1
/// letters such as ä ö ü ß stay, other letters fold to their base, emoji are dropped.
pub fn ascii_fold(s: &str, limit: usize) -> String {
    crate::text::board_text(s, limit)
}

/// Bytes received/sent in one tick to KiB/s. `dt` is floored like stats.py (1 ms).
pub fn net_rate_kbs(bytes: u64, dt_secs: f64) -> f64 {
    bytes as f64 / dt_secs.max(0.001) / KIB
}

/// Used share of a volume in percent: (total - available) / total.
pub fn disk_percent(total: u64, available: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    total.saturating_sub(available) as f64 / total as f64 * 100.0
}

fn clamp_pct(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn round_to(x: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (x * f).round_ties_even() / f // Python round() ties-to-even
}

/// stats.py used `SystemDrive` + "\\" on Windows; the root filesystem elsewhere.
fn system_drive() -> String {
    #[cfg(windows)]
    {
        let base = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        format!("{base}\\")
    }
    #[cfg(not(windows))]
    {
        "/".to_string()
    }
}

/// GPU sampling. A dedicated std thread owns the PDH/DXGI handles (they are not Send), and
/// publishes the latest reading into a shared slot that the async loop reads.
mod gpu {
    #![cfg_attr(not(windows), allow(dead_code))]

    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::ascii_fold;

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct GpuFields {
        pub util: f64,
        pub vram_used_bytes: Option<u64>,
        pub vram_total_bytes: Option<u64>,
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct GpuState {
        pub name: String,
        pub fields: Option<GpuFields>,
    }

    #[derive(Clone, Debug, PartialEq)]
    pub struct AdapterInfo {
        pub luid: u64,
        pub name: String,
        pub vram_total_bytes: u64,
    }

    pub type Shared = Arc<Mutex<GpuState>>;

    pub fn snapshot(shared: &Shared) -> GpuState {
        shared.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Starts the sampler thread. The thread exits once the returned handle is dropped.
    #[cfg(windows)]
    pub fn start() -> Shared {
        let shared: Shared = Arc::default();
        let worker = shared.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("gpu-pdh".into())
            .spawn(move || platform::worker(worker))
        {
            log::warn!("GPU sampler thread failed: {e}");
        }
        shared
    }

    #[cfg(not(windows))]
    pub fn start() -> Shared {
        Arc::default()
    }

    /// Packs a DXGI LUID (LowPart, HighPart) into one key.
    pub fn luid_key(low: u32, high: i32) -> u64 {
        ((high as u32 as u64) << 32) | low as u64
    }

    /// Reads the LUID out of a PDH instance name such as
    /// `pid_1234_luid_0x00000000_0x0000C2A0_phys_0_eng_0_engtype_3D`.
    pub fn parse_luid(instance: &str) -> Option<u64> {
        let rest = instance.split("luid_0x").nth(1)?;
        let (high, rest) = split_hex(rest)?;
        let rest = rest.strip_prefix("_0x")?;
        let (low, _) = split_hex(rest)?;
        Some(luid_key(low, high as i32))
    }

    fn split_hex(s: &str) -> Option<(u32, &str)> {
        let end = s.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(s.len());
        if end == 0 {
            return None;
        }
        let value = u32::from_str_radix(&s[..end], 16).ok()?;
        Some((value, &s[end..]))
    }

    /// Sums counter values per LUID. Instances without a LUID are skipped.
    pub fn sum_by_luid(items: &[(String, f64)]) -> HashMap<u64, f64> {
        let mut sums = HashMap::new();
        for (instance, value) in items {
            if let Some(luid) = parse_luid(instance) {
                *sums.entry(luid).or_insert(0.0) += value;
            }
        }
        sums
    }

    /// The LUID with the highest sum. Ties go to the smaller LUID.
    pub fn busiest(sums: &HashMap<u64, f64>) -> Option<(u64, f64)> {
        sums.iter()
            .map(|(k, v)| (*k, *v))
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
    }

    pub fn match_adapter(adapters: &[AdapterInfo], luid: u64) -> Option<&AdapterInfo> {
        adapters.iter().find(|a| a.luid == luid)
    }

    /// Drop the vendor word the board has no room for ("NVIDIA ", "AMD ").
    pub fn short_gpu_name(name: &str) -> String {
        let n = name.trim();
        n.strip_prefix("NVIDIA ")
            .or_else(|| n.strip_prefix("AMD "))
            .unwrap_or(n)
            .to_string()
    }

    /// DXGI_ADAPTER_DESC1.Description is a NUL-terminated UTF-16 buffer.
    pub fn wide_to_string(wide: &[u16]) -> String {
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        String::from_utf16_lossy(&wide[..end])
    }

    /// Turns the summed engine load of the busiest adapter into a GPU reading.
    pub fn reading(
        util_sums: &HashMap<u64, f64>,
        mem_sums: &HashMap<u64, f64>,
        adapters: &[AdapterInfo],
    ) -> GpuState {
        let Some((luid, load)) = busiest(util_sums) else {
            return GpuState::default();
        };
        let adapter = match_adapter(adapters, luid);
        GpuState {
            name: adapter.map(|a| ascii_fold(&a.name, 40)).unwrap_or_default(),
            fields: Some(GpuFields {
                util: load.clamp(0.0, 100.0),
                vram_used_bytes: mem_sums.get(&luid).map(|v| v.max(0.0) as u64),
                vram_total_bytes: adapter.map(|a| a.vram_total_bytes),
            }),
        }
    }

    #[cfg(windows)]
    mod platform {
        use std::sync::Arc;
        use std::time::Duration;

        use ::windows::core::{w, PCWSTR};
        use ::windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
        };
        use ::windows::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
            PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_CSTATUS_NEW_DATA,
            PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER,
            PDH_HQUERY, PDH_MORE_DATA,
        };

        use super::{
            luid_key, reading, short_gpu_name, wide_to_string, AdapterInfo, GpuState, Shared,
        };

        const PDH_NO_DATA: u32 = 0x800007D5;

        pub fn worker(shared: Shared) {
            let mut probe = match Probe::open() {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("GPU stats unavailable: {e:#}");
                    return;
                }
            };
            let mut logged = false;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if Arc::strong_count(&shared) == 1 {
                    break; // the stats collector is gone
                }
                let state = match probe.read() {
                    Ok(state) => state,
                    Err(e) => {
                        if !logged {
                            log::warn!("GPU read failed: {e:#}");
                            logged = true;
                        }
                        GpuState::default()
                    }
                };
                if let Ok(mut slot) = shared.lock() {
                    *slot = state;
                }
            }
        }

        struct Probe {
            query: PDH_HQUERY,
            util: PDH_HCOUNTER,
            mem: PDH_HCOUNTER,
            adapters: Vec<AdapterInfo>,
        }

        impl Probe {
            fn open() -> anyhow::Result<Self> {
                let adapters = enum_adapters()?;
                let mut query = PDH_HQUERY::default();
                check(
                    unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) },
                    "PdhOpenQueryW",
                )?;
                // Owns the query from here on, so Drop closes it on any error below.
                let mut probe = Probe {
                    query,
                    util: PDH_HCOUNTER::default(),
                    mem: PDH_HCOUNTER::default(),
                    adapters,
                };
                check(
                    unsafe {
                        PdhAddEnglishCounterW(
                            probe.query,
                            w!(r"\GPU Engine(*engtype_3D)\Utilization Percentage"),
                            0,
                            &mut probe.util,
                        )
                    },
                    "add GPU Engine counter",
                )?;
                check(
                    unsafe {
                        PdhAddEnglishCounterW(
                            probe.query,
                            w!(r"\GPU Adapter Memory(*)\Dedicated Usage"),
                            0,
                            &mut probe.mem,
                        )
                    },
                    "add GPU Adapter Memory counter",
                )?;
                // Rate counters need a first collection before any value is readable.
                check(
                    unsafe { PdhCollectQueryData(probe.query) },
                    "PdhCollectQueryData",
                )?;
                Ok(probe)
            }

            fn read(&mut self) -> anyhow::Result<GpuState> {
                check(
                    unsafe { PdhCollectQueryData(self.query) },
                    "PdhCollectQueryData",
                )?;
                let util = counter_array(self.util)?;
                let mem = counter_array(self.mem)?;
                Ok(reading(
                    &super::sum_by_luid(&util),
                    &super::sum_by_luid(&mem),
                    &self.adapters,
                ))
            }
        }

        impl Drop for Probe {
            fn drop(&mut self) {
                let _ = unsafe { PdhCloseQuery(self.query) };
            }
        }

        fn check(status: u32, what: &str) -> anyhow::Result<()> {
            if status == 0 {
                Ok(())
            } else {
                Err(anyhow::anyhow!("{what} failed: 0x{status:08X}"))
            }
        }

        fn counter_array(counter: PDH_HCOUNTER) -> anyhow::Result<Vec<(String, f64)>> {
            // The instance list can grow between the size query and the read: retry.
            for _ in 0..3 {
                match counter_array_once(counter)? {
                    Some(v) => return Ok(v),
                    None => continue,
                }
            }
            Ok(Vec::new())
        }

        /// None when the buffer turned out too small (PDH_MORE_DATA on the read).
        fn counter_array_once(counter: PDH_HCOUNTER) -> anyhow::Result<Option<Vec<(String, f64)>>> {
            let mut size = 0u32;
            let mut count = 0u32;
            let st = unsafe {
                PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
            };
            if st == PDH_NO_DATA {
                return Ok(Some(Vec::new()));
            }
            if st != PDH_MORE_DATA {
                check(st, "PdhGetFormattedCounterArrayW(size)")?;
                return Ok(Some(Vec::new()));
            }
            // The buffer holds the item structs followed by their name strings.
            let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let n = size as usize / item + 1;
            let mut items: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> =
                (0..n).map(|_| Default::default()).collect();
            let mut bytes = (n * item) as u32;
            let st = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE,
                    &mut bytes,
                    &mut count,
                    Some(items.as_mut_ptr()),
                )
            };
            if st == PDH_MORE_DATA {
                return Ok(None);
            }
            check(st, "PdhGetFormattedCounterArrayW")?;
            let mut out = Vec::new();
            for it in items.iter().take(count as usize) {
                let status = it.FmtValue.CStatus;
                if status != PDH_CSTATUS_VALID_DATA && status != PDH_CSTATUS_NEW_DATA {
                    continue;
                }
                let name = unsafe { it.szName.to_string() }.unwrap_or_default();
                let value = unsafe { it.FmtValue.Anonymous.doubleValue };
                out.push((name, value));
            }
            Ok(Some(out))
        }

        fn enum_adapters() -> anyhow::Result<Vec<AdapterInfo>> {
            let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
            let mut out = Vec::new();
            let mut i = 0u32;
            while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
                // One unreadable adapter must not switch GPU stats off for all.
                let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
                    i += 1;
                    continue;
                };
                if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) == 0 {
                    out.push(AdapterInfo {
                        luid: luid_key(desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart),
                        // Like stats.py drops "NVIDIA ": the board shows "Radeon RX ...".
                        name: short_gpu_name(&wide_to_string(&desc.Description)),
                        vram_total_bytes: desc.DedicatedVideoMemory as u64,
                    });
                }
                i += 1;
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::gpu::{self, AdapterInfo, GpuFields, GpuState};
    use super::*;
    use serde_json::json;

    const UTIL_INSTANCE: &str = "pid_4242_luid_0x00000000_0x0000C2A0_phys_0_eng_3_engtype_3D";
    const OTHER_INSTANCE: &str = "pid_77_luid_0x00000000_0x00001234_phys_0_eng_0_engtype_3D";

    #[test]
    fn parse_luid_reads_high_and_low_parts() {
        assert_eq!(gpu::parse_luid(UTIL_INSTANCE), Some(0xC2A0));
        assert_eq!(
            gpu::parse_luid("luid_0x00000001_0x00000002_phys_0"),
            Some((1u64 << 32) | 2)
        );
    }

    #[test]
    fn parse_luid_rejects_names_without_luid() {
        assert_eq!(gpu::parse_luid("pid_1_engtype_3D"), None);
        assert_eq!(gpu::parse_luid("luid_0x00000000_"), None);
        assert_eq!(gpu::parse_luid("luid_0xZZ_0x1"), None);
    }

    #[test]
    fn luid_key_handles_negative_high_part() {
        assert_eq!(gpu::luid_key(5, -1), (0xFFFF_FFFFu64 << 32) | 5);
        assert_eq!(
            gpu::parse_luid("luid_0xFFFFFFFF_0x00000005_phys_0"),
            Some(gpu::luid_key(5, -1))
        );
    }

    #[test]
    fn sum_by_luid_adds_instances_per_adapter() {
        let items = vec![
            (UTIL_INSTANCE.to_string(), 10.0),
            (UTIL_INSTANCE.replace("eng_3", "eng_4"), 5.5),
            (OTHER_INSTANCE.to_string(), 40.0),
            ("no-luid-here".to_string(), 99.0),
        ];
        let sums = gpu::sum_by_luid(&items);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums[&0xC2A0], 15.5);
        assert_eq!(sums[&0x1234], 40.0);
    }

    #[test]
    fn busiest_picks_max_and_breaks_ties_low() {
        let mut sums = std::collections::HashMap::new();
        sums.insert(10u64, 30.0);
        sums.insert(20u64, 70.0);
        assert_eq!(gpu::busiest(&sums), Some((20, 70.0)));
        sums.insert(5u64, 70.0);
        assert_eq!(gpu::busiest(&sums), Some((5, 70.0)));
        assert_eq!(gpu::busiest(&Default::default()), None);
    }

    #[test]
    fn match_adapter_finds_by_luid() {
        let adapters = vec![
            AdapterInfo {
                luid: 1,
                name: "a".into(),
                vram_total_bytes: 1,
            },
            AdapterInfo {
                luid: 2,
                name: "b".into(),
                vram_total_bytes: 2,
            },
        ];
        assert_eq!(
            gpu::match_adapter(&adapters, 2).map(|a| a.name.as_str()),
            Some("b")
        );
        assert!(gpu::match_adapter(&adapters, 3).is_none());
    }

    #[test]
    fn wide_to_string_stops_at_nul() {
        let wide: Vec<u16> = "Radeon RX".encode_utf16().chain([0, 0x41, 0x42]).collect();
        assert_eq!(gpu::wide_to_string(&wide), "Radeon RX");
        let no_nul: Vec<u16> = "AB".encode_utf16().collect();
        assert_eq!(gpu::wide_to_string(&no_nul), "AB");
    }

    #[test]
    fn reading_uses_busiest_adapter_and_its_vram() {
        let adapters = vec![AdapterInfo {
            luid: 0xC2A0,
            name: "AMD Radeon \u{2013} RX 7800".into(),
            vram_total_bytes: 16 * 1024 * 1024 * 1024,
        }];
        let util = gpu::sum_by_luid(&[(UTIL_INSTANCE.to_string(), 130.0)]);
        let mem = gpu::sum_by_luid(&[("luid_0x00000000_0x0000C2A0_phys_0".to_string(), 2.0 * GIB)]);
        let state = gpu::reading(&util, &mem, &adapters);
        assert_eq!(state.name, "AMD Radeon \u{2013} RX 7800");
        assert_eq!(
            state.fields,
            Some(GpuFields {
                util: 100.0,
                vram_used_bytes: Some(2 * 1024 * 1024 * 1024),
                vram_total_bytes: Some(16 * 1024 * 1024 * 1024),
            })
        );
    }

    #[test]
    fn reading_without_engines_is_empty() {
        let state = gpu::reading(&Default::default(), &Default::default(), &[]);
        assert_eq!(state, GpuState::default());
    }

    #[test]
    fn ascii_fold_keeps_what_the_font_draws() {
        assert_eq!(
            ascii_fold("Caf\u{00E9}\u{2019}s  CPU\t\u{2014}  x", 40),
            "Caf\u{00E9}\u{2019}s CPU \u{2014} x"
        );
        assert_eq!(ascii_fold("\u{2026}", 40), "\u{2026}");
        assert_eq!(ascii_fold("abcdef", 3), "abc");
        assert_eq!(ascii_fold("", 40), "");
    }

    #[test]
    fn cpu_name_clean_strips_brand_noise() {
        assert_eq!(
            cpu_name_clean("AMD Ryzen 7 7800X3D 8-Core Processor @ 4.2GHz"),
            "AMD Ryzen 7 7800X3D 8-Core"
        );
        assert_eq!(
            cpu_name_clean("Intel(R) Core(TM) i7-9700 CPU @ 3.00GHz"),
            "Intel Core i7-9700"
        );
    }

    #[test]
    fn net_rate_and_disk_percent() {
        assert_eq!(net_rate_kbs(2048, 1.0), 2.0);
        assert_eq!(net_rate_kbs(1024, 0.0), 1024.0 / 0.001 / KIB);
        assert_eq!(disk_percent(100, 25), 75.0);
        assert_eq!(disk_percent(0, 0), 0.0);
    }

    fn sample(gpu: Option<GpuFields>) -> StatsSample {
        StatsSample {
            cpu: 12.345,
            ram_used_bytes: (3.456 * GIB) as u64,
            ram_total_bytes: (31.96 * GIB) as u64,
            net_down_kbs: 123.456,
            net_up_kbs: 7.04,
            disk_pct: 61.25,
            cpu_name: "AMD Ryzen 7".into(),
            gpu_name: if gpu.is_some() {
                "AMD Radeon".into()
            } else {
                String::new()
            },
            gpu,
        }
    }

    #[test]
    fn build_json_without_gpu_omits_gpu_fields() {
        let v = build_stats_json(&sample(None));
        assert_eq!(
            v,
            json!({
                "type": "stats",
                "cpu": 12.3,
                "ram_used": 3.46,
                "ram_total": 32.0,
                "net_down": 123.5,
                "net_up": 7.0,
                "disk": 61.2,
                "cpu_name": "AMD Ryzen 7",
                "gpu_name": ""
            })
        );
        let obj = v.as_object().unwrap();
        for key in ["gpu", "gpu_temp", "vram_used", "vram_total", "cpu_temp"] {
            assert!(!obj.contains_key(key), "unexpected key {key}");
        }
    }

    #[test]
    fn build_json_with_gpu_adds_gpu_fields_in_stats_py_units() {
        let v = build_stats_json(&sample(Some(GpuFields {
            util: 42.04,
            vram_used_bytes: Some((5.678 * GIB) as u64),
            vram_total_bytes: Some((15.96 * GIB) as u64),
        })));
        assert_eq!(v["gpu"], json!(42.0));
        assert_eq!(v["vram_used"], json!(5.68));
        assert_eq!(v["vram_total"], json!(16.0));
        assert_eq!(v["gpu_name"], json!("AMD Radeon"));
    }

    #[test]
    fn build_json_gpu_without_vram_reading_keeps_only_util() {
        let v = build_stats_json(&sample(Some(GpuFields {
            util: 150.0,
            vram_used_bytes: None,
            vram_total_bytes: None,
        })));
        assert_eq!(v["gpu"], json!(100.0));
        assert!(v.get("vram_used").is_none());
        assert!(v.get("vram_total").is_none());
    }

    #[test]
    fn snapshot_of_fresh_slot_has_no_gpu() {
        let shared: gpu::Shared = Default::default();
        assert_eq!(gpu::snapshot(&shared), GpuState::default());
    }

    #[test]
    fn system_drive_is_root_off_windows() {
        #[cfg(not(windows))]
        assert_eq!(system_drive(), "/");
        #[cfg(windows)]
        assert!(system_drive().ends_with('\\'));
    }

    #[test]
    fn short_gpu_name_drops_vendor() {
        assert_eq!(
            gpu::short_gpu_name("AMD Radeon RX 7800 XT"),
            "Radeon RX 7800 XT"
        );
        assert_eq!(
            gpu::short_gpu_name("NVIDIA GeForce RTX 4070"),
            "GeForce RTX 4070"
        );
        assert_eq!(
            gpu::short_gpu_name("Intel(R) Arc(TM) A770"),
            "Intel(R) Arc(TM) A770"
        );
    }
}
