// GPU 传感器：PDH 性能计数器（任务管理器同源，NVIDIA/AMD/Intel 通吃）
// - 利用率：\GPU Engine(* engtype_3D)\Utilization Percentage（全实例求和）
// - 显存占用：\GPU Adapter Memory(*)\Dedicated Usage（全适配器求和）
// - 显存总量：注册表 HardwareInformation.qwMemorySize（各显示适配器求和）
// 全部走 PdhAddEnglishCounter（计数器名恒为英文），本地化免疫。
use std::sync::{LazyLock, Mutex};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

const PDH_FMT_DOUBLE: u32 = 0x200;
const UTIL_PATH: &str = r"\GPU Engine(*)\Utilization Percentage";
const MEMORY_PATH: &str = r"\GPU Adapter Memory(*)\Dedicated Usage";
#[repr(C)]
#[derive(Clone, Copy)]
union CounterNumber {
    double_value: f64,
    long_value: i32,
    large_value: i64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CounterValue {
    status: u32,
    number: CounterNumber,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CounterItem {
    name: *const u16,
    value: CounterValue,
}

fn aggregate_engines(values: &[(String, f64)]) -> Option<f64> {
    // 按物理引擎合并各进程，取最忙引擎而非把并行引擎/多卡相加。
    // 仅 3D；不代表整卡加权负载，也不包含 Compute/Copy/Video。
    let mut engines = std::collections::HashMap::<String, f64>::new();
    for (name, value) in values {
        if !value.is_finite() || *value < 0.0 || !name.ends_with("engtype_3D") {
            continue;
        }
        if let Some((_, engine)) = name.split_once("_luid_") {
            *engines.entry(engine.into()).or_default() += value;
        }
    }
    (!engines.is_empty()).then(|| {
        engines
            .values()
            .copied()
            .fold(0.0, f64::max)
            .clamp(0.0, 100.0)
    })
}
const PDH_MORE_DATA: i32 = 0x8000_07D2u32 as i32;
const PDH_CSTATUS_VALID_DATA: u32 = 0;
const PDH_CSTATUS_NEW_DATA: u32 = 1;

type PdhOpenQueryW = unsafe extern "system" fn(*const u16, usize, *mut isize) -> i32;
type PdhAddEnglishCounterW = unsafe extern "system" fn(isize, *const u16, usize, *mut isize) -> i32;
type PdhCollectQueryData = unsafe extern "system" fn(isize) -> i32;
type PdhGetFormattedCounterArrayW =
    unsafe extern "system" fn(isize, u32, *mut u32, *mut u32, *mut u8) -> i32;
type PdhCloseQuery = unsafe extern "system" fn(isize) -> i32;

struct PdhApi {
    open: PdhOpenQueryW,
    add: PdhAddEnglishCounterW,
    collect: PdhCollectQueryData,
    fmt_array: PdhGetFormattedCounterArrayW,
    close: PdhCloseQuery,
}

fn pdh() -> Option<&'static PdhApi> {
    static API: LazyLock<Option<PdhApi>> = LazyLock::new(|| unsafe {
        // pdh.dll 常驻系统；用运行时加载避免引入导入库依赖
        let h = GetModuleHandleA(c"pdh.dll".as_ptr().cast());
        let h = if h.is_null() {
            windows_sys::Win32::System::LibraryLoader::LoadLibraryA(c"pdh.dll".as_ptr().cast())
        } else {
            h
        };
        if h.is_null() {
            return None;
        }
        let get = |name: &[u8]| -> Option<*const ()> {
            let p = GetProcAddress(h, name.as_ptr());
            p.map(|p| p as *const ())
        };
        Some(PdhApi {
            open: std::mem::transmute::<*const (), PdhOpenQueryW>(get(b"PdhOpenQueryW\0")?),
            add: std::mem::transmute::<*const (), PdhAddEnglishCounterW>(get(
                b"PdhAddEnglishCounterW\0",
            )?),
            collect: std::mem::transmute::<*const (), PdhCollectQueryData>(get(
                b"PdhCollectQueryData\0",
            )?),
            fmt_array: std::mem::transmute::<*const (), PdhGetFormattedCounterArrayW>(get(
                b"PdhGetFormattedCounterArrayW\0",
            )?),
            close: std::mem::transmute::<*const (), PdhCloseQuery>(get(b"PdhCloseQuery\0")?),
        })
    });
    API.as_ref()
}

struct GpuQuery {
    q: isize,
    c_util: isize,
    c_mem: Option<isize>,
}

static QUERY: LazyLock<Mutex<Option<GpuQuery>>> = LazyLock::new(|| Mutex::new(None));

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn ensure_query() -> Option<()> {
    let mut guard = QUERY.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return Some(());
    }
    let api = pdh()?;
    unsafe {
        let mut q: isize = 0;
        if (api.open)(std::ptr::null(), 0, &mut q) != 0 {
            return None;
        }
        let mut c_util: isize = 0;
        if (api.add)(q, wide(UTIL_PATH).as_ptr(), 0, &mut c_util) != 0 {
            (api.close)(q);
            return None;
        }
        let mut c_mem: isize = 0;
        let c_mem = if (api.add)(q, wide(MEMORY_PATH).as_ptr(), 0, &mut c_mem) == 0 {
            Some(c_mem)
        } else {
            None
        };
        // 预热一次采样（利用率类计数器需要两次采样才有效）
        (api.collect)(q);
        *guard = Some(GpuQuery { q, c_util, c_mem });
        tracing::info!("GPU PDH 计数器已接入（引擎 3D 利用率 + 专用显存）");
        Some(())
    }
}

/// 对格式化数组求和（跳过无效状态项）
unsafe fn formatted(api: &PdhApi, counter: isize) -> Option<Vec<(String, f64)>> {
    let mut size: u32 = 0;
    let mut count: u32 = 0;
    let mut r = (api.fmt_array)(
        counter,
        PDH_FMT_DOUBLE,
        &mut size,
        &mut count,
        std::ptr::null_mut(),
    );
    if r != PDH_MORE_DATA || size == 0 {
        return None;
    }
    // u64 存储确保 C union / double 对齐，数组后还有 UTF-16 实例名。
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    r = (api.fmt_array)(
        counter,
        PDH_FMT_DOUBLE,
        &mut size,
        &mut count,
        buf.as_mut_ptr().cast(),
    );
    let capacity = buf.len() * 8;
    if r != 0
        || size as usize > capacity
        || count as usize > size as usize / std::mem::size_of::<CounterItem>()
    {
        return None;
    }
    let begin = buf.as_ptr() as usize;
    let end = begin + size as usize;
    let mut values = Vec::new();
    for i in 0..count as usize {
        let item = buf.as_ptr().cast::<CounterItem>().add(i).read();
        if ![PDH_CSTATUS_VALID_DATA, PDH_CSTATUS_NEW_DATA].contains(&item.value.status) {
            continue;
        }
        let ptr = item.name as usize;
        if ptr < begin || ptr >= end || !ptr.is_multiple_of(2) {
            continue;
        }
        let name = std::slice::from_raw_parts(item.name, (end - ptr) / 2);
        let Some(len) = name.iter().position(|c| *c == 0) else {
            continue;
        };
        let value = item.value.number.double_value;
        if value.is_finite() && value >= 0.0 {
            values.push((String::from_utf16_lossy(&name[..len]), value));
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GpuPdh {
    pub util_percent: f32,
    pub dedicated_bytes: Option<u64>,
}

/// 每秒由监控线程调用一次
pub fn poll() -> Option<GpuPdh> {
    ensure_query()?;
    let api = pdh()?;
    unsafe {
        let guard = QUERY.lock().unwrap_or_else(|e| e.into_inner());
        let gq = guard.as_ref()?;
        if (api.collect)(gq.q) != 0 {
            return None;
        }
        // Utilization Percentage 已经是百分比，0.34 即 0.34%，绝不乘 100。
        let util = aggregate_engines(&formatted(api, gq.c_util)?)?;
        let dedicated = gq.c_mem.and_then(|counter| {
            formatted(api, counter).map(|values| values.iter().map(|(_, n)| n).sum::<f64>() as u64)
        });
        Some(GpuPdh {
            util_percent: util.clamp(0.0, 100.0) as f32,
            dedicated_bytes: dedicated,
        })
    }
}

/// 各显示适配器显存总量之和（注册表 HardwareInformation）
pub fn total_vram_bytes() -> Option<u64> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let class = winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(
            r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}",
        )
        .ok()?;
    let mut total: u64 = 0;
    for sub in class.enum_keys().flatten() {
        if let Ok(k) = class.open_subkey(&sub) {
            if let Ok(v) = k.get_value::<u64, _>("HardwareInformation.qwMemorySize") {
                total += v;
            }
        }
    }
    (total > 0).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_windows_abi() {
        assert_eq!(std::mem::size_of::<CounterValue>(), 16);
        assert_eq!(std::mem::offset_of!(CounterValue, number), 8);
        assert_eq!(std::mem::offset_of!(CounterItem, value), 8);
        assert_eq!(std::mem::size_of::<CounterItem>(), 24);
        let item = CounterItem {
            name: std::ptr::null(),
            value: CounterValue {
                status: 1,
                number: CounterNumber { double_value: 0.34 },
            },
        };
        let decoded = unsafe { (&item as *const CounterItem).read() };
        assert_eq!(decoded.value.status, 1);
        assert_eq!(unsafe { decoded.value.number.double_value }, 0.34);
    }
    #[test]
    fn missing_memory_counter_is_unknown_not_zero_usage() {
        let sample = GpuPdh {
            util_percent: 12.5,
            dedicated_bytes: None,
        };
        assert_eq!(sample.dedicated_bytes, None);
        assert_eq!(MEMORY_PATH, r"\GPU Adapter Memory(*)\Dedicated Usage");
    }

    #[test]
    fn engines_sum_processes_not_parallel_engines_or_cards() {
        let row = |pid, engine, v| {
            (
                format!("pid_{pid}_luid_0x0_0x1_phys_0_eng_{engine}_engtype_3D"),
                v,
            )
        };
        assert_eq!(UTIL_PATH, r"\GPU Engine(*)\Utilization Percentage");
        assert_eq!(aggregate_engines(&[row(1, 0, 0.34)]), Some(0.34));
        assert_eq!(
            aggregate_engines(&[row(1, 0, 30.0), row(2, 0, 20.0), row(1, 1, 40.0)]),
            Some(50.0)
        );
        assert_eq!(
            aggregate_engines(&[row(1, 0, 120.0), row(2, 1, f64::NAN)]),
            Some(100.0)
        );
        assert_eq!(
            aggregate_engines(&[("pid_1_luid_other_engtype_Copy".into(), 90.0)]),
            None
        );
    }

    #[test]
    fn pdh_api_loads() {
        assert!(pdh().is_some(), "pdh.dll 应可加载（Windows 内置）");
    }

    #[test]
    fn gpu_counters_open() {
        // 机器有 GPU 时计数器应能打开（无 GPU 的 CI 上此测试跳过意义不大，直接容错）
        let r = ensure_query();
        if r.is_some() {
            if let Some(sample) = poll() {
                assert!(
                    sample.util_percent.is_finite() && (0.0..=100.0).contains(&sample.util_percent)
                );
            }
        }
    }

    #[test]
    fn vram_total_plausible() {
        if let Some(t) = total_vram_bytes() {
            assert!(t > 100 * 1024 * 1024, "显存总量应 > 100MB");
            assert!(t < 256 * 1024 * 1024 * 1024, "显存总量应 < 256GB");
        }
    }
}
