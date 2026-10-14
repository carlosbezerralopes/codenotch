#Requires -Version 7
[CmdletBinding()]
param([switch]$Stream, [switch]$Once)

$ErrorActionPreference = 'Stop'
$WarningPreference = 'SilentlyContinue'
$ProgressPreference = 'SilentlyContinue'
if (-not $IsWindows) { throw 'Este coletor requer Windows.' }
if ($Stream -and $Once) { throw 'Use -Stream ou -Once.' }

# Kernel timings, available RAM and DXGI identity avoid localized counter names.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
namespace ActivityMonitor {
 [StructLayout(LayoutKind.Sequential)] public struct MemoryStatus {
  public uint Length, Load;
  public ulong TotalPhys, AvailPhys, TotalPage, AvailPage, TotalVirtual, AvailVirtual, AvailExtended;
 }
 [StructLayout(LayoutKind.Sequential)] public struct Luid { public uint Low; public int High; }
 [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct AdapterDesc {
  [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Description;
  public uint VendorId, DeviceId, SubSysId, Revision;
  public UIntPtr DedicatedVideoMemory, DedicatedSystemMemory, SharedSystemMemory;
  public Luid Luid; public uint Flags;
 }
 [ComImport, Guid("29038f61-3839-4626-91fd-086879011a05"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
 interface IAdapter {
  [PreserveSig] int SetPrivateData(IntPtr a,uint b,IntPtr c);
  [PreserveSig] int SetPrivateDataInterface(IntPtr a,IntPtr b);
  [PreserveSig] int GetPrivateData(IntPtr a,IntPtr b,IntPtr c);
  [PreserveSig] int GetParent(IntPtr a,IntPtr b);
  [PreserveSig] int EnumOutputs(uint a,out IntPtr b);
  [PreserveSig] int GetDesc(IntPtr a);
  [PreserveSig] int CheckInterfaceSupport(IntPtr a,IntPtr b);
  [PreserveSig] int GetDesc1(out AdapterDesc desc);
 }
 [ComImport, Guid("770aae78-f26f-4dba-a829-253c83d1b387"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
 interface IFactory {
  [PreserveSig] int SetPrivateData(IntPtr a,uint b,IntPtr c);
  [PreserveSig] int SetPrivateDataInterface(IntPtr a,IntPtr b);
  [PreserveSig] int GetPrivateData(IntPtr a,IntPtr b,IntPtr c);
  [PreserveSig] int GetParent(IntPtr a,IntPtr b);
  [PreserveSig] int EnumAdapters(uint a,out IntPtr b);
  [PreserveSig] int MakeWindowAssociation(IntPtr a,uint b);
  [PreserveSig] int GetWindowAssociation(out IntPtr a);
  [PreserveSig] int CreateSwapChain(IntPtr a,IntPtr b,out IntPtr c);
  [PreserveSig] int CreateSoftwareAdapter(IntPtr a,out IntPtr b);
  [PreserveSig] int EnumAdapters1(uint index,out IAdapter adapter);
  [PreserveSig] bool IsCurrent();
 }
 public static class Native {
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GetSystemTimes(out long idle,out long kernel,out long user);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GlobalMemoryStatusEx(ref MemoryStatus memory);
  [DllImport("dxgi.dll")] static extern int CreateDXGIFactory1(ref Guid iid,[MarshalAs(UnmanagedType.Interface)] out IFactory factory);
  public static AdapterDesc[] Adapters() {
   Guid iid=new Guid("770aae78-f26f-4dba-a829-253c83d1b387"); IFactory factory;
   Marshal.ThrowExceptionForHR(CreateDXGIFactory1(ref iid,out factory));
   var result=new List<AdapterDesc>();
   try { for(uint i=0;;i++) {
    IAdapter adapter; int hr=factory.EnumAdapters1(i,out adapter);
    if(hr==unchecked((int)0x887A0002)) break;
    Marshal.ThrowExceptionForHR(hr);
    try { AdapterDesc desc; Marshal.ThrowExceptionForHR(adapter.GetDesc1(out desc)); if((desc.Flags&2)==0) result.Add(desc); }
    finally { Marshal.ReleaseComObject(adapter); }
   }} finally { Marshal.ReleaseComObject(factory); }
   return result.ToArray();
  }
 }
}
'@

$script:Clock = [Diagnostics.Stopwatch]::StartNew()
$script:PreviousCpu = $null
$script:PreviousDisk = @{}
$script:PreviousGpu = @{}
$script:PreviousNetwork = @{}
$script:InventoryIssues = [Collections.Generic.List[string]]::new()
$script:GpuInventory = @()
$script:DiskInventory = @{}
$script:NetworkInventory = @{}
$script:CpuSubtitle = "$([Environment]::ProcessorCount) processadores lógicos"

function Add-Issue($List, [string]$Area, $Failure) {
    if ($List.Count -lt 6) {
        $type = if ($Failure) { $Failure.Exception.GetType().Name } else { 'Indisponivel' }
        $List.Add("${Area}: $type")
    }
}
function Format-Number([double]$Number, [int]$Decimals = 1) {
    $Number.ToString("F$Decimals", [Globalization.CultureInfo]::GetCultureInfo('pt-BR'))
}
function Format-Rate([double]$Bytes) {
    if ($Bytes -ge 1MB) { return "$(Format-Number ($Bytes / 1MB)) MB/s" }
    if ($Bytes -ge 1KB) { return "$(Format-Number ($Bytes / 1KB)) KB/s" }
    return "$(Format-Number $Bytes 0) B/s"
}
function Limit-Percent([double]$Value) { [math]::Round([math]::Min(100.0, [math]::Max(0.0, $Value)), 2) }
function New-Resource([string]$Id, [string]$Kind, [string]$Name, [string]$Subtitle = '', $Vendor = $null) {
    [ordered]@{ id=$Id; kind=$Kind; name=$Name; subtitle=$Subtitle; vendor=$Vendor; percent=$null;
        value='—'; detail='Sem leitura'; read_bps=$null; write_bps=$null; temperature_c=$null; status='unavailable' }
}
function Luid-Key([int]$High, [uint32]$Low) { '0x{0:x8}_0x{1:x8}' -f ([uint32]([long]$High -band 0xffffffffL)), $Low }
function Normalize-Guid([string]$Value) { $Value.Trim('{}').ToLowerInvariant() }
function Valid-Interval($Previous, [double]$Now) { $null -ne $Previous -and ($Now - $Previous.at) -gt 0 -and ($Now - $Previous.at) -le 30 }

try {
    $cpuInfo = @(Get-CimInstance Win32_Processor -Property Name,MaxClockSpeed)
    if ($cpuInfo.Count) { $script:CpuSubtitle = "$($cpuInfo[0].Name.Trim()) · $(Format-Number ($cpuInfo[0].MaxClockSpeed / 1000)) GHz máx." }
} catch { Add-Issue $script:InventoryIssues 'Inventário CPU' $_ }
try {
    $script:GpuInventory = @([ActivityMonitor.Native]::Adapters() | ForEach-Object {
        $vendor = switch ($_.VendorId) { 0x10de {'NVIDIA'} 0x1002 {'AMD'} 0x8086 {'Intel'} default {'GPU'} }
        [pscustomobject]@{ key=(Luid-Key $_.Luid.High $_.Luid.Low); name=$_.Description; vendor=$vendor }
    })
} catch { Add-Issue $script:InventoryIssues 'Inventário GPU' $_ }
try {
    $media = @{}
    try { foreach ($disk in @(Get-PhysicalDisk -ErrorAction Stop)) {
        $media[[string]$disk.DeviceId] = @{type=[string]$disk.MediaType; model=[string]$disk.FriendlyName}
    } } catch { }
    foreach ($disk in @(Get-CimInstance Win32_DiskDrive -Property Index,Model)) {
        $index = [string]$disk.Index
        $kind = if ($media[$index] -and $media[$index].type -in @('SSD','HDD') -and $media[$index].model.Trim() -eq $disk.Model.Trim()) { $media[$index].type } else { 'Disco' }
        $script:DiskInventory[$index] = [pscustomobject]@{ name="$kind $index"; model=[string]$disk.Model }
    }
} catch { Add-Issue $script:InventoryIssues 'Inventário disco' $_ }
try {
    # PhysicalAdapter alone also includes VPN drivers on this machine. Require a hardware PNP path.
    foreach ($nic in @(Get-CimInstance Win32_NetworkAdapter -Property GUID,PNPDeviceID,NetConnectionID,Name)) {
        if ($nic.GUID -and $nic.PNPDeviceID -match '^(PCI|USB)\\') {
            $script:NetworkInventory[(Normalize-Guid $nic.GUID)] = [pscustomobject]@{ name=[string]$nic.NetConnectionID; model=[string]$nic.Name }
        }
    }
} catch { Add-Issue $script:InventoryIssues 'Inventário rede' $_ }

function Get-Snapshot {
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $resources = [Collections.Generic.List[object]]::new()
    $issues = [Collections.Generic.List[string]]::new()
    foreach ($issue in $script:InventoryIssues) { $issues.Add($issue) }

    $cpu = New-Resource 'cpu' 'cpu' 'CPU' $script:CpuSubtitle
    try {
        $idle=0L; $kernel=0L; $user=0L
        if (-not [ActivityMonitor.Native]::GetSystemTimes([ref]$idle,[ref]$kernel,[ref]$user)) { throw 'GetSystemTimes' }
        $now = $script:Clock.Elapsed.TotalSeconds
        if (Valid-Interval $script:PreviousCpu $now) {
            $prior = $script:PreviousCpu
            $total = ($kernel-$prior.kernel)+($user-$prior.user)
            $idleDelta = $idle-$prior.idle
            if ($total -gt 0 -and $idleDelta -ge 0 -and $idleDelta -le $total) {
                $cpu.percent=Limit-Percent (100*(1-$idleDelta/$total)); $cpu.value="$(Format-Number $cpu.percent 0)%"
                $cpu.status='ok'; $cpu.detail='Uso médio de todos os processadores lógicos'
            } else { $cpu.status='warming'; $cpu.detail='Contador reiniciado' }
        } else { $cpu.status='warming'; $cpu.detail='Aguardando próxima leitura' }
        $script:PreviousCpu=@{at=$now;idle=$idle;kernel=$kernel;user=$user}
    } catch { $script:PreviousCpu=$null; Add-Issue $issues 'CPU' $_ }
    $resources.Add($cpu)

    $ram = New-Resource 'memory' 'memory' 'RAM'
    try {
        $memory = [ActivityMonitor.MemoryStatus]::new()
        $memory.Length=[uint32][Runtime.InteropServices.Marshal]::SizeOf($memory)
        if (-not [ActivityMonitor.Native]::GlobalMemoryStatusEx([ref]$memory) -or $memory.TotalPhys -eq 0) { throw 'GlobalMemoryStatusEx' }
        $used=$memory.TotalPhys-$memory.AvailPhys
        $ram.percent=Limit-Percent (100*$used/$memory.TotalPhys); $ram.value="$(Format-Number $ram.percent 0)%"
        $ram.subtitle="$(Format-Number ($used/1GB)) / $(Format-Number ($memory.TotalPhys/1GB)) GB"
        $ram.detail="$(Format-Number ($memory.AvailPhys/1GB)) GB disponíveis"; $ram.status='ok'
    } catch { Add-Issue $issues 'RAM' $_ }
    $resources.Add($ram)

    try {
        $diskRows = @(Get-CimInstance Win32_PerfRawData_PerfDisk_PhysicalDisk -Filter "Name!='_Total'" -Property Name,PercentIdleTime,PercentIdleTime_Base,DiskReadBytesPersec,DiskWriteBytesPersec,Timestamp_PerfTime,Frequency_PerfTime)
        $nextDisk=@{}
        foreach ($row in $diskRows) {
            if ($row.Name -notmatch '^(\d+)(?:\s|$)') { continue }
            $index=$Matches[1]; $meta=$script:DiskInventory[$index]
            $disk=New-Resource "disk-$index" 'disk' $(if($meta){$meta.name}else{"Disco $index"}) $(if($meta){$meta.model}else{$row.Name})
            $now=$script:Clock.Elapsed.TotalSeconds; $prior=$script:PreviousDisk[$row.Name]
            $disk.status='warming'; $disk.detail='Aguardando próxima leitura'
            if (Valid-Interval $prior $now) {
                $old=$prior.row
                $base=[double]([decimal]$row.PercentIdleTime_Base-[decimal]$old.PercentIdleTime_Base)
                $idleDelta=[double]([decimal]$row.PercentIdleTime-[decimal]$old.PercentIdleTime)
                $read=[double]([decimal]$row.DiskReadBytesPersec-[decimal]$old.DiskReadBytesPersec)
                $write=[double]([decimal]$row.DiskWriteBytesPersec-[decimal]$old.DiskWriteBytesPersec)
                $ticks=[double]([decimal]$row.Timestamp_PerfTime-[decimal]$old.Timestamp_PerfTime)
                if ($base -gt 0 -and $idleDelta -ge 0 -and $read -ge 0 -and $write -ge 0 -and $ticks -gt 0 -and $row.Frequency_PerfTime -gt 0) {
                    $dt=$ticks/[double]$row.Frequency_PerfTime
                    $disk.percent=Limit-Percent (100*(1-$idleDelta/$base)); $disk.value="$(Format-Number $disk.percent 0)%"
                    $disk.read_bps=[math]::Round($read/$dt,2); $disk.write_bps=[math]::Round($write/$dt,2)
                    $disk.detail="Leitura $(Format-Rate $disk.read_bps) · escrita $(Format-Rate $disk.write_bps)"; $disk.status='ok'
                } else { $disk.detail='Contador reiniciado' }
            }
            $nextDisk[$row.Name]=@{at=$now;row=$row}; $resources.Add($disk)
        }
        $script:PreviousDisk=$nextDisk
        if (-not $diskRows.Count) { $resources.Add((New-Resource 'disk' 'disk' 'Disco')) }
    } catch { $script:PreviousDisk=@{}; $resources.Add((New-Resource 'disk' 'disk' 'Disco')); Add-Issue $issues 'Disco' $_ }

    $gpuById=@{}; $nextGpu=@{}; $gpuQueryOk=$false
    try {
        $rows=@(Get-CimInstance Win32_PerfRawData_GPUPerformanceCounters_GPUEngine -Property Name,RunningTime,Timestamp_Sys100NS)
        $now=$script:Clock.Elapsed.TotalSeconds
        foreach ($row in $rows) {
            if ($row.Name -notmatch '^pid_\d+_luid_(0x[0-9a-f]+)_(0x[0-9a-f]+)_phys_(\d+)_eng_(\d+)_engtype_') { continue }
            $key=Luid-Key ([Convert]::ToInt32($Matches[1].Substring(2),16)) ([Convert]::ToUInt32($Matches[2].Substring(2),16))
            $engine="$($Matches[3]):$($Matches[4])"
            if (-not $gpuById.ContainsKey($key)) { $gpuById[$key]=@{engines=@{};valid=0;seen=0;partial=$false} }
            $group=$gpuById[$key]; $group.seen++
            $prior=$script:PreviousGpu[$row.Name]
            if (Valid-Interval $prior $now) {
                $dt=[double]([decimal]$row.Timestamp_Sys100NS-[decimal]$prior.row.Timestamp_Sys100NS)
                $run=[double]([decimal]$row.RunningTime-[decimal]$prior.row.RunningTime)
                if ($dt -gt 0 -and $run -ge 0) {
                    if (-not $group.engines.ContainsKey($engine)) { $group.engines[$engine]=0.0 }
                    $group.engines[$engine]+=100*$run/$dt; $group.valid++
                } else { $group.partial=$true }
            } else { $group.partial=$true }
            $nextGpu[$row.Name]=@{at=$now;row=$row}
        }
        foreach ($name in $script:PreviousGpu.Keys) {
            if (-not $nextGpu.ContainsKey($name) -and $name -match '_luid_(0x[0-9a-f]+)_(0x[0-9a-f]+)_phys_') {
                $key=Luid-Key ([Convert]::ToInt32($Matches[1].Substring(2),16)) ([Convert]::ToUInt32($Matches[2].Substring(2),16))
                if ($gpuById.ContainsKey($key)) { $gpuById[$key].partial=$true }
            }
        }
        $gpuQueryOk=$true; $script:PreviousGpu=$nextGpu
    } catch { $script:PreviousGpu=@{}; Add-Issue $issues 'GPU' $_ }
    foreach ($meta in $script:GpuInventory) {
        $gpu=New-Resource "gpu-$($meta.key)" 'gpu' $meta.vendor $meta.name $meta.vendor
        $group=$gpuById[$meta.key]
        if ($gpuQueryOk -and $group -and $group.valid -gt 0) {
            $busiest=($group.engines.Values | Measure-Object -Maximum).Maximum
            $gpu.percent=Limit-Percent $busiest; $gpu.value="$(Format-Number $gpu.percent 0)%"; $gpu.status='ok'
            $gpu.detail=if($group.partial){'Engine mais ocupado · leitura parcial após mudança de instâncias'}else{'Engine mais ocupado'}
        } elseif ($gpuQueryOk -and $group) { $gpu.status='warming'; $gpu.detail='Aguardando próxima leitura' }
        $resources.Add($gpu)
    }
    if (-not $script:GpuInventory.Count) { $resources.Add((New-Resource 'gpu' 'gpu' 'GPU')) }

    try {
        $nextNetwork=@{}; $count=0
        foreach ($nic in [Net.NetworkInformation.NetworkInterface]::GetAllNetworkInterfaces()) {
            $key=Normalize-Guid $nic.Id; $meta=$script:NetworkInventory[$key]
            if (-not $meta -or $nic.NetworkInterfaceType -notin @([Net.NetworkInformation.NetworkInterfaceType]::Ethernet,[Net.NetworkInformation.NetworkInterfaceType]::Wireless80211) -or $nic.OperationalStatus -ne [Net.NetworkInformation.OperationalStatus]::Up) { continue }
            $count++
            $name=if($nic.NetworkInterfaceType -eq [Net.NetworkInformation.NetworkInterfaceType]::Ethernet){'Ethernet'}else{'Wi-Fi'}
            $network=New-Resource "network-$key" 'network' $name $meta.model
            $stat=$nic.GetIPStatistics(); $now=$script:Clock.Elapsed.TotalSeconds; $prior=$script:PreviousNetwork[$key]
            $network.status='warming'; $network.detail='Aguardando próxima leitura'
            if (Valid-Interval $prior $now) {
                $rx=$stat.BytesReceived-$prior.rx; $tx=$stat.BytesSent-$prior.tx; $dt=$now-$prior.at
                if ($rx -ge 0 -and $tx -ge 0) {
                    $network.read_bps=[math]::Round($rx/$dt,2); $network.write_bps=[math]::Round($tx/$dt,2)
                    # Full-duplex links can send and receive at link speed simultaneously.
                    if ($nic.Speed -gt 0) { $network.percent=Limit-Percent (100*[math]::Max($network.read_bps,$network.write_bps)*8/$nic.Speed) }
                    $network.value=Format-Rate $network.read_bps
                    $network.detail="Recebido $(Format-Rate $network.read_bps) · enviado $(Format-Rate $network.write_bps)"; $network.status='ok'
                } else { $network.detail='Contador reiniciado' }
            }
            $nextNetwork[$key]=@{at=$now;rx=$stat.BytesReceived;tx=$stat.BytesSent}; $resources.Add($network)
        }
        $script:PreviousNetwork=$nextNetwork
        if (-not $count) { $resource=New-Resource 'network' 'network' 'Rede'; $resource.detail='Sem interface física conectada'; $resources.Add($resource) }
    } catch { $script:PreviousNetwork=@{}; $resources.Add((New-Resource 'network' 'network' 'Rede')); Add-Issue $issues 'Rede' $_ }

    [ordered]@{schema_version=1;sampled_at=[DateTimeOffset]::UtcNow.ToString('o');collection_ms=[math]::Round($timer.Elapsed.TotalMilliseconds,2);platform='windows';resources=$resources.ToArray();issues=$issues.ToArray()}
}

if ($Stream) {
    while ($null -ne ($line=[Console]::ReadLine())) {
        if ($line.Trim() -eq 'sample') { Get-Snapshot | ConvertTo-Json -Depth 6 -Compress }
        elseif ($line.Trim() -eq 'quit') { break }
        else { [Console]::Error.WriteLine('Comando desconhecido; esperado sample ou quit.') }
    }
} else { Get-Snapshot | ConvertTo-Json -Depth 6 -Compress }
