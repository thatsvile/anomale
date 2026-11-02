use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct SysSnapshot {
    pub volume: Option<Volume>,
    pub battery: Option<Battery>,
    pub memory: Memory,
    pub cpu_usage: u32,
    pub cpu_temp_c: Option<u32>,
    pub gpu: Option<Gpu>,
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub percent: u32,
    pub muted: bool,
}

#[derive(Debug, Clone)]
pub struct Battery {
    pub percent: u32,
    pub charging: bool,
    pub status: String,
    pub watts: Option<f32>,
    pub seconds: Option<u32>,
    pub energy_wh: Option<(f32, f32)>,
}

#[derive(Debug, Clone, Default)]
pub struct Memory {
    pub used_kib: u64,
    pub total_kib: u64,
    pub percent: u32,
    pub swap: Option<Swap>,
}

#[derive(Debug, Clone)]
pub struct Swap {
    pub used_kib: u64,
    pub total_kib: u64,
    pub percent: u32,
}

#[derive(Debug, Clone)]
pub struct Gpu {
    pub usage: u32,
    pub temp_c: Option<u32>,
}

const VOLUME_QUIET: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub struct SysSampler {
    prev_cpu: Option<(u64, u64)>,
    volume: Option<Volume>,
    last_user_volume: Option<Instant>,
}

impl SysSampler {
    pub fn sample(&mut self) -> SysSnapshot {
        if should_refresh_volume(
            self.volume.is_some(),
            self.last_user_volume.map(|t| t.elapsed()),
        ) {
            self.volume = read_volume();
        }
        SysSnapshot {
            volume: self.volume.clone(),
            battery: read_battery(),
            memory: read_memory(),
            cpu_usage: self.read_cpu_usage(),
            cpu_temp_c: read_cpu_temp(),
            gpu: read_gpu(),
        }
    }

    pub fn note_volume(&mut self, volume: Volume) {
        self.volume = Some(volume);
        self.last_user_volume = Some(Instant::now());
    }

    fn read_cpu_usage(&mut self) -> u32 {
        let Some((idle, total)) = parse_proc_stat() else {
            return 0;
        };
        let pct = if let Some((prev_idle, prev_total)) = self.prev_cpu {
            let d_idle = idle.saturating_sub(prev_idle);
            let d_total = total.saturating_sub(prev_total);
            if d_total == 0 {
                0
            } else {
                (((d_total - d_idle) * 100) / d_total) as u32
            }
        } else {
            0
        };
        self.prev_cpu = Some((idle, total));
        pct.min(100)
    }
}

fn parse_proc_stat() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    if parts.next()? != "cpu" {
        return None;
    }
    let nums: Vec<u64> = parts.filter_map(|n| n.parse().ok()).collect();
    if nums.len() < 4 {
        return None;
    }
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    let total = nums.iter().sum();
    Some((idle, total))
}

fn read_memory() -> Memory {
    fs::read_to_string("/proc/meminfo")
        .ok()
        .map(|text| parse_meminfo(&text))
        .unwrap_or_default()
}

pub(crate) fn parse_meminfo(text: &str) -> Memory {
    let mut total = 0u64;
    let mut available = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = parse_kib(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = parse_kib(rest);
        } else if let Some(rest) = line.strip_prefix("SwapTotal:") {
            swap_total = parse_kib(rest);
        } else if let Some(rest) = line.strip_prefix("SwapFree:") {
            swap_free = parse_kib(rest);
        }
    }
    let used = total.saturating_sub(available);
    let percent = if total == 0 {
        0
    } else {
        ((used * 100) / total) as u32
    };
    let swap = if swap_total == 0 {
        None
    } else {
        let swap_used = swap_total.saturating_sub(swap_free);
        Some(Swap {
            used_kib: swap_used,
            total_kib: swap_total,
            percent: ((swap_used * 100) / swap_total) as u32,
        })
    };
    Memory {
        used_kib: used,
        total_kib: total,
        percent: percent.min(100),
        swap,
    }
}

pub(crate) fn format_kib(kib: u64) -> String {
    const MIB: f64 = 1024.0;
    const GIB: f64 = 1024.0 * 1024.0;
    let n = kib as f64;
    if n >= GIB {
        let g = n / GIB;
        if g >= 10.0 {
            format!("{g:.0}G")
        } else {
            format!("{g:.1}G")
        }
    } else if n >= MIB {
        format!("{:.0}M", n / MIB)
    } else {
        format!("{kib}K")
    }
}

fn parse_kib(s: &str) -> u64 {
    s.split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn read_battery() -> Option<Battery> {
    let dir = fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in dir.flatten() {
        let path = entry.path();
        let kind = read_trimmed(&path.join("type")).unwrap_or_default();
        if !kind.eq_ignore_ascii_case("Battery") {
            continue;
        }
        let percent: u32 = read_trimmed(&path.join("capacity"))?.parse().ok()?;
        let status = read_trimmed(&path.join("status")).unwrap_or_else(|| "Unknown".into());
        let charging =
            status.eq_ignore_ascii_case("Charging") || status.eq_ignore_ascii_case("Full");
        let voltage = read_u64(&path.join("voltage_now"));
        let energy_now = read_u64(&path.join("energy_now"))
            .or_else(|| charge_to_energy(read_u64(&path.join("charge_now"))?, voltage?));
        let energy_full = read_u64(&path.join("energy_full"))
            .or_else(|| charge_to_energy(read_u64(&path.join("charge_full"))?, voltage?));
        let power = read_u64(&path.join("power_now")).or_else(|| {
            Some(read_u64(&path.join("current_now"))?.saturating_mul(voltage?) / 1_000_000)
        });
        let watts = power.filter(|&p| p > 0).map(|p| p as f32 / 1_000_000.0);
        let energy_wh = match (energy_now, energy_full) {
            (Some(now), Some(full)) => Some((now as f32 / 1_000_000.0, full as f32 / 1_000_000.0)),
            _ => None,
        };
        let seconds = battery_seconds(&status, &path, energy_now, energy_full, power);
        return Some(Battery {
            percent: percent.min(100),
            charging,
            status,
            watts,
            seconds,
            energy_wh,
        });
    }
    None
}

fn battery_seconds(
    status: &str,
    path: &Path,
    energy_now: Option<u64>,
    energy_full: Option<u64>,
    power: Option<u64>,
) -> Option<u32> {
    let estimated = || match (energy_now, energy_full, power) {
        (Some(now), Some(full), Some(watts)) => estimate_seconds(now, full, watts, status),
        _ => None,
    };
    if status.eq_ignore_ascii_case("Charging") {
        read_u64(&path.join("time_to_full_now"))
            .map(u64_secs)
            .or_else(estimated)
    } else if status.eq_ignore_ascii_case("Discharging") {
        read_u64(&path.join("time_to_empty_now"))
            .map(u64_secs)
            .or_else(estimated)
    } else {
        None
    }
}

fn u64_secs(v: u64) -> u32 {
    v.min(u64::from(u32::MAX)) as u32
}

fn charge_to_energy(charge_uah: u64, voltage_uv: u64) -> Option<u64> {
    if voltage_uv == 0 {
        return None;
    }
    Some(charge_uah.saturating_mul(voltage_uv) / 1_000_000)
}

pub(crate) fn estimate_seconds(
    energy_now: u64,
    energy_full: u64,
    power_uw: u64,
    status: &str,
) -> Option<u32> {
    if power_uw == 0 {
        return None;
    }
    let remaining = if status.eq_ignore_ascii_case("Charging") {
        energy_full.saturating_sub(energy_now)
    } else if status.eq_ignore_ascii_case("Discharging") {
        energy_now
    } else {
        return None;
    };
    Some(((remaining as u128 * 3600) / power_uw as u128).min(u128::from(u32::MAX)) as u32)
}

pub(crate) fn format_secs(secs: u32) -> String {
    let mins = secs / 60;
    if mins < 1 {
        return "<1m".into();
    }
    let hours = mins / 60;
    let mins = mins % 60;
    if hours == 0 {
        format!("{mins}m")
    } else if mins == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h{mins}m")
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

fn read_cpu_temp() -> Option<u32> {
    let hwmon = Path::new("/sys/class/hwmon");
    let mut fallback = None;
    let entries = fs::read_dir(hwmon).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = read_trimmed(&path.join("name")).unwrap_or_default();
        let preferred = name == "k10temp" || name == "coretemp" || name == "zenpower";
        if let Some(temp) = read_hwmon_temp(&path, preferred) {
            if preferred {
                return Some(temp);
            }
            fallback = fallback.or(Some(temp));
        }
    }
    fallback
}

fn read_hwmon_temp(chip: &Path, prefer_package: bool) -> Option<u32> {
    if prefer_package {
        for idx in 1..16 {
            let label = read_trimmed(&chip.join(format!("temp{idx}_label"))).unwrap_or_default();
            if label.contains("Tctl")
                || label.contains("Package")
                || label.eq_ignore_ascii_case("Tdie")
            {
                if let Some(temp) = millic_to_c(&chip.join(format!("temp{idx}_input"))) {
                    return Some(temp);
                }
            }
        }
    }
    millic_to_c(&chip.join("temp1_input"))
}

fn millic_to_c(path: &Path) -> Option<u32> {
    let raw: i64 = read_trimmed(path)?.parse().ok()?;
    Some((raw / 1000).clamp(0, 200) as u32)
}

fn read_gpu() -> Option<Gpu> {
    read_gpu_sysfs().or_else(read_gpu_nvidia)
}

fn read_gpu_sysfs() -> Option<Gpu> {
    let drm = fs::read_dir("/sys/class/drm").ok()?;
    for entry in drm.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = entry.path().join("device");
        let usage = read_trimmed(&device.join("gpu_busy_percent"))
            .and_then(|s| s.parse::<u32>().ok())
            .or_else(|| {
                read_trimmed(&device.join("gpu_busy_percent_vis"))
                    .and_then(|s| s.parse::<u32>().ok())
            });
        let temp = read_gpu_hwmon_temp(&device);
        if let Some(usage) = usage {
            return Some(Gpu {
                usage: usage.min(100),
                temp_c: temp,
            });
        }
        if temp.is_some() {
            return Some(Gpu {
                usage: 0,
                temp_c: temp,
            });
        }
    }
    None
}

fn read_gpu_hwmon_temp(device: &Path) -> Option<u32> {
    let hwmon_dir = device.join("hwmon");
    let entries = fs::read_dir(hwmon_dir).ok()?;
    for entry in entries.flatten() {
        if let Some(temp) = millic_to_c(&entry.path().join("temp1_input")) {
            return Some(temp);
        }
    }
    None
}

fn read_gpu_nvidia() -> Option<Gpu> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let line = line.lines().next()?;
    let mut parts = line.split(',');
    let usage = parts.next()?.trim().parse().ok()?;
    let temp = parts.next().and_then(|s| s.trim().parse().ok());
    Some(Gpu {
        usage,
        temp_c: temp,
    })
}

fn should_refresh_volume(cached: bool, since_user_adjust: Option<Duration>) -> bool {
    if !cached {
        return true;
    }
    match since_user_adjust {
        None => true,
        Some(age) => age >= VOLUME_QUIET,
    }
}

fn read_volume() -> Option<Volume> {
    read_volume_wpctl().or_else(read_volume_pactl)
}

fn read_volume_wpctl() -> Option<Volume> {
    let out = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_wpctl(&String::from_utf8_lossy(&out.stdout))
}

fn parse_wpctl(text: &str) -> Option<Volume> {
    let muted = text.contains("MUTED");
    let frac = text
        .split_whitespace()
        .find_map(|tok| tok.parse::<f32>().ok())?;
    Some(Volume {
        percent: (frac * 100.0).round().clamp(0.0, 150.0) as u32,
        muted,
    })
}

fn read_volume_pactl() -> Option<Volume> {
    let out = Command::new("pactl")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .output()
        .ok()?;
    let muted = String::from_utf8_lossy(&out.stdout).contains("yes");
    let out = Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let percent = text
        .split_whitespace()
        .find_map(|tok| tok.strip_suffix('%')?.parse::<u32>().ok())?;
    Some(Volume { percent, muted })
}

pub fn toggle_mute() -> Option<Volume> {
    let ok = Command::new("wpctl")
        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = Command::new("pactl")
            .args(["set-sink-mute", "@DEFAULT_SINK@", "toggle"])
            .status();
    }
    read_volume()
}

pub fn set_volume_delta(delta: i32) -> Option<Volume> {
    if delta == 0 {
        return read_volume();
    }
    let bump = delta.unsigned_abs();
    let arg = if delta > 0 {
        format!("{bump}%+")
    } else {
        format!("{bump}%-")
    };
    let ok = Command::new("wpctl")
        .args(["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", &arg])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let sign = if delta > 0 { "+" } else { "-" };
        let _ = Command::new("pactl")
            .args([
                "set-sink-volume",
                "@DEFAULT_SINK@",
                &format!("{sign}{bump}%"),
            ])
            .status();
    }
    read_volume()
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::parse_wpctl;

    #[test]
    fn parses_wpctl_volume() {
        let vol = parse_wpctl("Volume: 0.65\n").unwrap();
        assert_eq!(vol.percent, 65);
        assert!(!vol.muted);
        let muted = parse_wpctl("Volume: 0.40 [MUTED]\n").unwrap();
        assert!(muted.muted);
        assert_eq!(muted.percent, 40);
    }

    #[test]
    fn polls_volume_after_two_quiet_seconds() {
        use super::should_refresh_volume;
        use std::time::Duration;
        assert!(should_refresh_volume(false, None));
        assert!(should_refresh_volume(true, None));
        assert!(!should_refresh_volume(
            true,
            Some(Duration::from_millis(1999))
        ));
        assert!(should_refresh_volume(true, Some(Duration::from_secs(2))));
        assert!(should_refresh_volume(true, Some(Duration::from_secs(5))));
    }

    #[test]
    fn estimates_charge_time_and_formats() {
        use super::{estimate_seconds, format_secs};
        // 6.9 Wh remaining at 21.145 W ≈ 20 minutes
        let secs = estimate_seconds(32_020_000, 38_920_000, 21_145_000, "Charging").unwrap();
        assert!((18 * 60..22 * 60).contains(&secs));
        let empty = estimate_seconds(32_020_000, 38_920_000, 21_145_000, "Discharging").unwrap();
        assert!(empty > 3600);
        assert_eq!(format_secs(20), "<1m");
        assert_eq!(format_secs(20 * 60), "20m");
        assert_eq!(format_secs(2 * 3600 + 14 * 60), "2h14m");
        assert_eq!(format_secs(3 * 3600), "3h");
        assert!(estimate_seconds(1, 1, 0, "Charging").is_none());
    }

    #[test]
    fn parses_meminfo_and_omits_empty_swap() {
        use super::{format_kib, parse_meminfo};
        let mem = parse_meminfo(
            "MemTotal:        8000000 kB\n\
             MemAvailable:    2000000 kB\n\
             SwapTotal:       4000000 kB\n\
             SwapFree:        1000000 kB\n",
        );
        assert_eq!(mem.total_kib, 8_000_000);
        assert_eq!(mem.used_kib, 6_000_000);
        assert_eq!(mem.percent, 75);
        let swap = mem.swap.unwrap();
        assert_eq!(swap.total_kib, 4_000_000);
        assert_eq!(swap.used_kib, 3_000_000);
        assert_eq!(swap.percent, 75);
        let none = parse_meminfo(
            "MemTotal:        1000 kB\n\
             MemAvailable:     500 kB\n\
             SwapTotal:          0 kB\n\
             SwapFree:           0 kB\n",
        );
        assert_eq!(none.percent, 50);
        assert!(none.swap.is_none());
        assert_eq!(format_kib(512), "512K");
        assert_eq!(format_kib(2048), "2M");
        assert_eq!(format_kib(8_388_608), "8.0G");
    }
}
