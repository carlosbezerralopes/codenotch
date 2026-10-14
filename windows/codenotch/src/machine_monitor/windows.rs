use super::{byte_rates, cpu_percent, valid_elapsed, Resource, Snapshot};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, mem::size_of, time::Instant};
use ::windows::{core::{w, PCWSTR}, Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1},
    NetworkManagement::{IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211}, Ndis::IfOperStatusUp},
    Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        IO::DeviceIoControl,
        Ioctl::{DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery,
            STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, StorageDeviceProperty, StorageDeviceSeekPenaltyProperty},
        Performance::{PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
            PdhOpenQueryW, PDH_FMT, PDH_FMT_DOUBLE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_MORE_DATA},
        Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetSystemTimes,
    },
}};

fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }
fn utf16(text: &[u16]) -> String {
    String::from_utf16_lossy(&text[..text.iter().position(|value| *value == 0).unwrap_or(text.len())]).trim().into()
}
fn rate(value: f64) -> String {
    if value >= 1_048_576.0 { format!("{:.1} MB/s", value / 1_048_576.0) }
    else if value >= 1024.0 { format!("{:.1} KB/s", value / 1024.0) }
    else { format!("{value:.0} B/s") }
}
fn issue(issues: &mut Vec<String>, area: &str, code: impl std::fmt::Display) {
    if issues.len() < 12 { issues.push(format!("{area}: {code}")); }
}

struct PdhQuery {
    handle: isize,
    disk_idle: Option<isize>,
    disk_read: Option<isize>,
    disk_write: Option<isize>,
    gpu: Option<isize>,
    issues: Vec<String>,
}
impl Drop for PdhQuery {
    fn drop(&mut self) { unsafe { PdhCloseQuery(self.handle); } }
}
impl PdhQuery {
    fn new() -> Result<Self, u32> {
        let mut handle = 0;
        let code = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) };
        if code != 0 { return Err(code); }
        let mut result = Self { handle, disk_idle: None, disk_read: None, disk_write: None, gpu: None, issues: Vec::new() };
        result.disk_idle = result.add(r"\PhysicalDisk(*)\% Idle Time", "Disco/atividade");
        result.disk_read = result.add(r"\PhysicalDisk(*)\Disk Read Bytes/sec", "Disco/leitura");
        result.disk_write = result.add(r"\PhysicalDisk(*)\Disk Write Bytes/sec", "Disco/escrita");
        result.gpu = result.add(r"\GPU Engine(*)\Utilization Percentage", "GPU");
        Ok(result)
    }
    fn add(&mut self, path: &str, area: &str) -> Option<isize> {
        let path = wide(path);
        let mut counter = 0;
        let code = unsafe { PdhAddEnglishCounterW(self.handle, PCWSTR(path.as_ptr()), 0, &mut counter) };
        if code == 0 { Some(counter) } else { issue(&mut self.issues, area, format!("PDH {code:#x}")); None }
    }
    fn collect(&self) -> Result<(), u32> {
        let code = unsafe { PdhCollectQueryData(self.handle) };
        if code == 0 { Ok(()) } else { Err(code) }
    }
    fn rows(counter: Option<isize>) -> Result<BTreeMap<String, Option<f64>>, u32> {
        let counter = counter.ok_or(0xc0000bb9u32)?;
        // NOCAP100 lets per-PID GPU engines sum before the final percent is clamped.
        let format = PDH_FMT(PDH_FMT_DOUBLE.0 | 0x8000);
        for _ in 0..3 {
            let mut bytes = 0;
            let mut count = 0;
            let code = unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, None) };
            if code == 0 && count == 0 { return Ok(BTreeMap::new()); }
            if code != PDH_MORE_DATA { return Err(code); }
            if bytes == 0 || bytes > 8 * 1024 * 1024 { return Err(0xc0000bc6); }
            // PDH writes pointers/doubles; u64 storage provides the required alignment.
            let mut buffer = vec![0u64; (bytes as usize + 7) / 8];
            let capacity = buffer.len() * size_of::<u64>();
            let code = unsafe { PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count,
                Some(buffer.as_mut_ptr().cast())) };
            if code == PDH_MORE_DATA { continue; }
            if code != 0 { return Err(code); }
            if count as usize > capacity / size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() { return Err(0xc0000bc6); }
            let rows = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(), count as usize) };
            let mut result = BTreeMap::new();
            for row in rows {
                let offset = (row.szName.0 as usize).checked_sub(buffer.as_ptr() as usize);
                let Some(offset) = offset.filter(|offset| *offset < capacity && offset % 2 == 0) else { continue; };
                let units = (capacity - offset) / 2;
                let name = utf16(unsafe { std::slice::from_raw_parts(row.szName.0, units.min(2048)) });
                let value = unsafe { row.FmtValue.Anonymous.doubleValue };
                let valid = row.FmtValue.CStatus <= 1 && value.is_finite() && value >= 0.0;
                result.insert(name, valid.then_some(value));
            }
            return Ok(result);
        }
        Err(PDH_MORE_DATA)
    }
}

#[derive(Clone)]
struct Gpu { id: String, luid: (u32, u32), vendor: String, name: String }
fn gpu_inventory() -> Result<Vec<Gpu>, String> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|e| e.code().to_string())?;
    let mut result = Vec::new();
    let mut ordinals = BTreeMap::new();
    for index in 0..128 {
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code().0 as u32 == 0x887a0002 => break,
            Err(error) => return Err(error.code().to_string()),
        };
        let desc = unsafe { adapter.GetDesc1() }.map_err(|e| e.code().to_string())?;
        if desc.Flags & 2 != 0 { continue; }
        let identity = format!("{:04x}-{:04x}-{:08x}", desc.VendorId, desc.DeviceId, desc.SubSysId);
        let ordinal = ordinals.entry(identity.clone()).or_insert(0u32);
        let vendor = match desc.VendorId { 0x10de => "NVIDIA", 0x1002 => "AMD", 0x8086 => "Intel", _ => "GPU" };
        result.push(Gpu { id: format!("gpu-{identity}-{ordinal}"),
            luid: (desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart),
            vendor: vendor.into(), name: utf16(&desc.Description) });
        *ordinal += 1;
    }
    Ok(result)
}

// PID is deliberately discarded: usage is the busiest (physical GPU, engine), not a sum of engines.
fn gpu_engine(name: &str) -> Option<((u32, u32), (u32, u32))> {
    let name = name.split('#').next()?;
    let parts: Vec<_> = name.split('_').collect();
    if parts.len() < 10 || parts[0] != "pid" || parts[2] != "luid" || parts[5] != "phys" || parts[7] != "eng" || parts[9] != "engtype" { return None; }
    parts[1].parse::<u32>().ok()?;
    Some(((u32::from_str_radix(parts[3].strip_prefix("0x")?, 16).ok()?,
           u32::from_str_radix(parts[4].strip_prefix("0x")?, 16).ok()?),
          (parts[6].parse().ok()?, parts[8].parse().ok()?)))
}

fn gpu_usage(rows: &BTreeMap<String, Option<f64>>, luid: (u32, u32)) -> (Option<f64>, bool, bool) {
    let mut engines = BTreeMap::new();
    let mut seen = false;
    let mut partial = false;
    for (name, value) in rows {
        if let Some((adapter, engine)) = gpu_engine(name) {
            if adapter != luid { continue; }
            seen = true;
            match value {
                Some(value) => *engines.entry(engine).or_insert(0.0f64) += value,
                None => partial = true,
            }
        }
    }
    (engines.values().copied().reduce(f64::max).map(|value| value.clamp(0.0, 100.0)), partial, seen)
}

struct FileHandle(HANDLE);
impl Drop for FileHandle { fn drop(&mut self) { let _ = unsafe { CloseHandle(self.0) }; } }
#[derive(Clone)]
struct Disk { id: String, name: String, model: String }
fn device_query(handle: HANDLE, property: ::windows::Win32::System::Ioctl::STORAGE_PROPERTY_ID) -> Option<Vec<u8>> {
    let query = STORAGE_PROPERTY_QUERY { PropertyId: property, QueryType: PropertyStandardQuery, ..Default::default() };
    let mut bytes = vec![0u8; 8192];
    let mut returned = 0;
    unsafe { DeviceIoControl(handle, IOCTL_STORAGE_QUERY_PROPERTY, Some((&query as *const STORAGE_PROPERTY_QUERY).cast()),
        size_of::<STORAGE_PROPERTY_QUERY>() as u32, Some(bytes.as_mut_ptr().cast()), bytes.len() as u32,
        Some(&mut returned), None) }.ok()?;
    bytes.truncate(returned.min(bytes.len() as u32) as usize);
    Some(bytes)
}
fn storage_string(bytes: &[u8], offset: u32) -> String {
    if offset == 0 { return String::new(); }
    bytes.get(offset as usize..).map(|remaining| {
        let end = remaining.iter().position(|byte| *byte == 0).unwrap_or(remaining.len());
        String::from_utf8_lossy(&remaining[..end]).trim().to_string()
    }).unwrap_or_default()
}
fn disk_inventory(index: u32) -> Disk {
    let mut disk = Disk { id: format!("disk-{index}"), name: format!("Disco {index}"), model: String::new() };
    let path = wide(&format!(r"\\.\PhysicalDrive{index}"));
    let Ok(handle) = (unsafe { CreateFileW(PCWSTR(path.as_ptr()), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None,
        OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, HANDLE::default()) }) else { return disk; };
    let handle = FileHandle(handle);
    if let Some(bytes) = device_query(handle.0, StorageDeviceProperty).filter(|bytes| bytes.len() >= size_of::<STORAGE_DEVICE_DESCRIPTOR>()) {
        let desc = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>()) };
        disk.model = storage_string(&bytes, desc.ProductIdOffset);
        let serial = storage_string(&bytes, desc.SerialNumberOffset);
        if !serial.is_empty() {
            // A stable identity without exposing device serial numbers through UI IPC.
            let hash = Sha256::digest(format!("{}\0{serial}", disk.model).as_bytes());
            disk.id = format!("disk-{:x}", hash)[..21].to_string();
        }
    }
    if let Some(bytes) = device_query(handle.0, StorageDeviceSeekPenaltyProperty)
        .filter(|bytes| bytes.len() >= size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()) {
        let desc = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()) };
        if desc.Size as usize >= size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() {
            let media = if desc.IncursSeekPenalty.0 == 0 { "SSD" } else { "HDD" };
            disk.name = format!("{media} {index}");
        }
    }
    disk
}

struct IfTable(*mut MIB_IF_TABLE2);
impl Drop for IfTable { fn drop(&mut self) { unsafe { FreeMibTable(self.0.cast()); } } }
fn network_rows() -> Result<Vec<::windows::Win32::NetworkManagement::IpHelper::MIB_IF_ROW2>, u32> {
    let mut ptr = std::ptr::null_mut();
    let code = unsafe { GetIfTable2(&mut ptr) };
    if code.0 != 0 || ptr.is_null() { return Err(code.0); }
    let table = IfTable(ptr);
    let count = unsafe { (*table.0).NumEntries } as usize;
    if count > 4096 { return Err(13); }
    let rows = unsafe { std::slice::from_raw_parts((*table.0).Table.as_ptr(), count) };
    Ok(rows.iter().filter(|row| {
        let flags = row.InterfaceAndOperStatusFlags._bitfield;
        flags & 1 != 0 && flags & 2 == 0 && row.OperStatus == IfOperStatusUp &&
            [IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211].contains(&row.Type)
    }).copied().collect())
}

fn cpu_name() -> String {
    let mut bytes = [0u16; 512];
    let mut size = (bytes.len() * 2) as u32;
    let result = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
        w!("ProcessorNameString"), RRF_RT_REG_SZ, None, Some(bytes.as_mut_ptr().cast()), Some(&mut size)) };
    if result.0 == 0 { utf16(&bytes) } else { "Processadores lógicos".into() }
}
fn cpu_times() -> Option<(u64, u64, u64)> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    let ticks = |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Some((ticks(idle), ticks(kernel), ticks(user)))
}

pub(super) struct NativeCollector {
    pdh: Option<PdhQuery>,
    pdh_error: Option<u32>,
    cpu_name: String,
    previous_cpu: Option<((u64, u64, u64), Instant)>,
    previous_network: BTreeMap<String, ((u64, u64), Instant)>,
    last_sample: Option<Instant>,
    inventory_at: Instant,
    gpus: Vec<Gpu>,
    gpu_error: Option<String>,
    disks: BTreeMap<u32, Disk>,
}
impl NativeCollector {
    pub(super) fn new() -> Self {
        let (pdh, pdh_error) = match PdhQuery::new() { Ok(query) => (Some(query), None), Err(code) => (None, Some(code)) };
        let (gpus, gpu_error) = match gpu_inventory() { Ok(rows) => (rows, None), Err(error) => (Vec::new(), Some(error)) };
        Self { pdh, pdh_error, cpu_name: cpu_name(), previous_cpu: None, previous_network: BTreeMap::new(),
            last_sample: None, inventory_at: Instant::now(), gpus, gpu_error, disks: BTreeMap::new() }
    }
    pub(super) fn sample(&mut self) -> Snapshot {
        let started = Instant::now();
        if started.duration_since(self.inventory_at).as_secs() >= 60 {
            match gpu_inventory() { Ok(rows) => { self.gpus = rows; self.gpu_error = None; }, Err(error) => self.gpu_error = Some(error) }
            self.disks.clear();
            if self.pdh.is_none() || self.pdh.as_ref().is_some_and(|query| !query.issues.is_empty()) {
                match PdhQuery::new() { Ok(query) => { self.pdh = Some(query); self.pdh_error = None; self.last_sample = None; },
                    Err(code) => self.pdh_error = Some(code) }
            }
            self.inventory_at = started;
        }
        let mut snapshot = Snapshot::default();
        let mut cpu = Resource::new("cpu", "cpu", "CPU", &self.cpu_name);
        if let Some(times) = cpu_times() {
            cpu.warming();
            if let Some((prior, at)) = self.previous_cpu {
                if let Some(percent) = cpu_percent(prior, times, started.duration_since(at).as_secs_f64()) {
                    cpu.set_percent(percent); cpu.detail = "Uso médio de todos os processadores lógicos".into();
                }
            }
            self.previous_cpu = Some((times, started));
        } else { self.previous_cpu = None; issue(&mut snapshot.issues, "CPU", "GetSystemTimes indisponível"); }
        snapshot.resources.push(cpu);
        let mut ram = Resource::new("memory", "memory", "RAM", "");
        let mut memory = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if unsafe { GlobalMemoryStatusEx(&mut memory) }.is_ok() && memory.ullTotalPhys > 0 && memory.ullAvailPhys <= memory.ullTotalPhys {
            let total = memory.ullTotalPhys as f64; let free = memory.ullAvailPhys as f64; let used = total - free;
            ram.set_percent(100.0 * used / total);
            ram.subtitle = format!("{:.1} / {:.1} GB", used / 1_073_741_824.0, total / 1_073_741_824.0);
            ram.detail = format!("{:.1} GB disponíveis", free / 1_073_741_824.0);
        } else { issue(&mut snapshot.issues, "RAM", "GlobalMemoryStatusEx indisponível"); }
        snapshot.resources.push(ram);

        let interval_valid = self.last_sample.is_some_and(|previous| valid_elapsed(started.duration_since(previous).as_secs_f64()));
        let mut pdh_ok = false;
        let mut idle_rows = BTreeMap::new(); let mut read_rows = BTreeMap::new(); let mut write_rows = BTreeMap::new();
        let mut gpu_rows = BTreeMap::new(); let mut gpu_ok = false;
        if let Some(query) = &self.pdh {
            snapshot.issues.extend(query.issues.clone());
            match query.collect() {
                Ok(()) => {
                    pdh_ok = true;
                    for (counter, destination, area) in [
                        (query.disk_idle, &mut idle_rows, "Disco/atividade"), (query.disk_read, &mut read_rows, "Disco/leitura"),
                        (query.disk_write, &mut write_rows, "Disco/escrita"), (query.gpu, &mut gpu_rows, "GPU"),
                    ] {
                        if counter.is_none() { continue; }
                        match PdhQuery::rows(counter) {
                            Ok(rows) => { *destination = rows; if area == "GPU" { gpu_ok = true; } },
                            Err(code) => issue(&mut snapshot.issues, area, format!("PDH {code:#x}")),
                        }
                    }
                }, Err(code) => issue(&mut snapshot.issues, "Contadores", format!("PDH {code:#x}")),
            }
        } else if let Some(code) = self.pdh_error { issue(&mut snapshot.issues, "Contadores", format!("PDH {code:#x}")); }
        self.last_sample = if pdh_ok { Some(started) } else { None };
        for (name, idle) in &idle_rows {
            let Some(index) = name.split_whitespace().next().and_then(|part| part.parse::<u32>().ok()) else { continue; };
            let meta = self.disks.entry(index).or_insert_with(|| disk_inventory(index));
            let mut disk = Resource::new(&meta.id, "disk", &meta.name, if meta.model.is_empty() { name.as_str() } else { &meta.model });
            disk.warming();
            if interval_valid {
                if let Some(idle) = idle {
                    disk.set_percent(100.0 - idle.clamp(0.0, 100.0));
                    disk.read_bps = read_rows.get(name).copied().flatten();
                    disk.write_bps = write_rows.get(name).copied().flatten();
                    disk.detail = format!("Leitura {} · escrita {}", disk.read_bps.map(rate).unwrap_or_else(|| "—".into()),
                        disk.write_bps.map(rate).unwrap_or_else(|| "—".into()));
                }
            }
            snapshot.resources.push(disk);
        }
        if !snapshot.resources.iter().any(|resource| resource.kind == "disk") {
            snapshot.resources.push(Resource::new("disk", "disk", "Disco", ""));
        }
        if let Some(error) = &self.gpu_error { issue(&mut snapshot.issues, "Inventário GPU", error); }
        for (index, meta) in self.gpus.iter().enumerate() {
            let mut gpu = Resource::new(&meta.id, "gpu", format!("GPU {index}"), &meta.name);
            gpu.vendor = Some(meta.vendor.clone());
            let (percent, partial, seen) = gpu_usage(&gpu_rows, meta.luid);
            if gpu_ok && seen {
                gpu.warming();
                if interval_valid {
                    if let Some(percent) = percent {
                        gpu.set_percent(percent);
                        gpu.detail = if partial { "Engine mais ocupado · leitura parcial após mudança de instâncias" }
                            else { "Engine mais ocupado" }.into();
                    }
                }
            }
            snapshot.resources.push(gpu);
        }
        if self.gpus.is_empty() { snapshot.resources.push(Resource::new("gpu", "gpu", "GPU", "")); }

        let mut next_network = BTreeMap::new();
        match network_rows() {
            Ok(rows) => for row in rows {
                let id = format!("network-{:?}", row.InterfaceGuid).to_lowercase();
                let name = if row.Type == IF_TYPE_ETHERNET_CSMACD { "Ethernet" } else { "Wi-Fi" };
                let mut network = Resource::new(&id, "network", name, utf16(&row.Description));
                network.warming();
                let now = Instant::now();
                let counters = (row.InOctets, row.OutOctets);
                if let Some((previous, at)) = self.previous_network.get(&id) {
                    if let Some((rx, tx)) = byte_rates(*previous, counters, now.duration_since(*at).as_secs_f64()) {
                        network.read_bps = Some(rx); network.write_bps = Some(tx);
                        // Full duplex: take the larger directional fraction instead of adding both directions.
                        let rx_pct = (row.ReceiveLinkSpeed > 0).then(|| rx * 800.0 / row.ReceiveLinkSpeed as f64);
                        let tx_pct = (row.TransmitLinkSpeed > 0).then(|| tx * 800.0 / row.TransmitLinkSpeed as f64);
                        if let Some(percent) = match (rx_pct, tx_pct) { (Some(a), Some(b)) => Some(a.max(b)), _ => None } {
                            network.set_percent(percent);
                        }
                        network.value = rate(rx); network.status = "ok".into();
                        network.detail = format!("Recebido {} · enviado {}", rate(rx), rate(tx));
                    }
                }
                next_network.insert(id, (counters, now));
                snapshot.resources.push(network);
            }, Err(code) => issue(&mut snapshot.issues, "Rede", format!("GetIfTable2 {code}")),
        }
        self.previous_network = next_network;
        if !snapshot.resources.iter().any(|resource| resource.kind == "network") {
            let mut network = Resource::new("network", "network", "Rede", "");
            network.detail = "Sem interface física conectada".into(); snapshot.resources.push(network);
        }
        snapshot.collection_ms = started.elapsed().as_secs_f64() * 1000.0;
        snapshot.sampled_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_sums_processes_per_engine_then_takes_busiest_engine() {
        let rows = BTreeMap::from([
            ("pid_1_luid_0x00000000_0x00001234_phys_0_eng_0_engtype_3D".into(), Some(30.0)),
            ("pid_2_luid_0x00000000_0x00001234_phys_0_eng_0_engtype_3D".into(), Some(40.0)),
            ("pid_3_luid_0x00000000_0x00001234_phys_0_eng_1_engtype_Copy".into(), Some(50.0)),
            ("pid_4_luid_0x00000000_0x00001234_phys_0_eng_2_engtype_Video".into(), None),
            ("pid_5_luid_0x00000000_0x00009999_phys_0_eng_0_engtype_3D".into(), Some(100.0)),
        ]);
        assert_eq!(gpu_usage(&rows, (0, 0x1234)), (Some(70.0), true, true));
        assert_eq!(gpu_usage(&rows, (0, 0x7777)), (None, false, false));
        assert_eq!(gpu_engine("bogus"), None);
        assert_eq!(gpu_engine("pid_1_luid_0xffffffff_0x00001234_phys_1_eng_3_engtype_3D#1"), Some(((u32::MAX, 0x1234), (1, 3))));
    }
    #[test]
    fn unavailable_engines_do_not_become_zero_usage() {
        let rows = BTreeMap::from([("pid_1_luid_0x0_0x1234_phys_0_eng_0_engtype_3D".into(), None)]);
        assert_eq!(gpu_usage(&rows, (0, 0x1234)), (None, true, true));
        assert_eq!(gpu_usage(&BTreeMap::new(), (0, 0x1234)), (None, false, false));
    }
}
