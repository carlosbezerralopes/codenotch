#Requires -Version 7
<#
.SYNOPSIS
Tests and builds the Windows app and hook; optionally packages an unsigned NSIS installer.
.DESCRIPTION
Requires installed Rust/MSVC, Visual Studio C++ Build Tools and Node.js. Downloads
Cargo dependencies as needed; -Installer also downloads the pinned Tauri CLI/NSIS
tools. Does not install prerequisites, launch the app or change user configuration.
#>
[CmdletBinding()]
param([switch]$Installer)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if (-not $IsWindows) { throw 'This build requires Windows and PowerShell 7.' }

$windowsRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$runId = Get-Date -Format 'yyyyMMdd-HHmmss-ffff'
$runDirectory = Join-Path $windowsRoot "target/local-build/$runId"
New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
$logPath = Join-Path $runDirectory 'build.log'
$manifestPath = Join-Path $runDirectory 'manifest.json'
$started = [DateTimeOffset]::Now
$originalLocation = Get-Location
$environmentNames = @('PATH', 'TEMP', 'TMP', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD')
$originalEnvironment = @{}
foreach ($name in $environmentNames) {
    $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$runtimeTemp = Join-Path ([IO.Path]::GetTempPath()) 'codenotch/local-build'
New-Item -ItemType Directory -Path $runtimeTemp -Force | Out-Null
$env:TEMP = $runtimeTemp
$env:TMP = $runtimeTemp
$manifest = [ordered]@{
    started_at = $started.ToString('o')
    finished_at = $null
    status = 'running'
    installer_requested = [bool]$Installer
    windows_root = $windowsRoot
    log = $logPath
    artifacts = @()
}

function Write-BuildLog([string]$Message) {
    Write-Host $Message
    Add-Content -LiteralPath $logPath -Value $Message -Encoding utf8
}

function Set-BuildEnvironment {
    param([string]$Name, [AllowNull()][AllowEmptyString()][object]$Value)
    # On recent .NET, SetEnvironmentVariable(null) can leave an empty variable.
    # Cargo distinguishes an absent target from an empty target, which it rejects.
    if ($null -eq $Value) {
        Remove-Item -LiteralPath "Env:$Name" -ErrorAction SilentlyContinue
    } else {
        [Environment]::SetEnvironmentVariable($Name, [string]$Value, 'Process')
    }
}

function Invoke-BuildCommand {
    param([string]$File, [string[]]$Arguments, [switch]$Capture)
    Write-BuildLog "> $File $($Arguments -join ' ')"
    $lines = [Collections.Generic.List[string]]::new()
    & $File @Arguments 2>&1 | ForEach-Object {
        $line = $_.ToString()
        Write-BuildLog $line
        if ($Capture) { $lines.Add($line) }
    }
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) { throw "$File failed (exit $exitCode). See $logPath" }
    if ($Capture) { return $lines -join "`n" }
}

function Find-BuildTool([string]$Name, [string]$Fallback = '') {
    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) { return $command.Source }
    if ($Fallback -and (Test-Path -LiteralPath $Fallback -PathType Leaf)) { return $Fallback }
    throw "Missing $Name. Install the prerequisite described in windows/docs/local-build.md, then retry."
}

function Add-BuildArtifact([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Expected build artifact is missing: $Path" }
    $file = Get-Item -LiteralPath $Path
    $manifest.artifacts += [ordered]@{
        path = $file.FullName
        bytes = $file.Length
        sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
    }
}

try {
    Set-Location -LiteralPath $windowsRoot
    Write-BuildLog "Local build started $($started.ToString('o')). Log: $logPath"
    $cargoHomeBin = Join-Path $HOME '.cargo/bin'
    $cargo = Find-BuildTool 'cargo.exe' (Join-Path $cargoHomeBin 'cargo.exe')
    $rustup = Find-BuildTool 'rustup.exe' (Join-Path $cargoHomeBin 'rustup.exe')
    # Cargo and the bundler must resolve rustc/cargo from the same installed shim directory.
    $env:PATH = "$(Split-Path -Parent $cargo);$(Split-Path -Parent $rustup);$env:PATH"
    $rustc = Find-BuildTool 'rustc.exe' (Join-Path $cargoHomeBin 'rustc.exe')
    $node = Find-BuildTool 'node.exe'
    $npx = if ($Installer) { Find-BuildTool 'npx.cmd' } else { $null }
    Invoke-BuildCommand $cargo @('--version')
    Invoke-BuildCommand $rustup @('show', 'active-toolchain')
    $rustVersion = Invoke-BuildCommand $rustc @('-vV') -Capture
    if ($rustVersion -notmatch '(?m)^host: x86_64-pc-windows-msvc\s*$') {
        throw 'The active Rust toolchain must use host x86_64-pc-windows-msvc. Select an installed x64 MSVC toolchain before retrying.'
    }
    Invoke-BuildCommand $node @('--version')
    if ($Installer) { Invoke-BuildCommand $npx @('--version') }

    $programFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
    $vswhere = Find-BuildTool 'vswhere.exe' (Join-Path $programFilesX86 'Microsoft Visual Studio/Installer/vswhere.exe')
    $cppInstallations = @()
    foreach ($workload in @('Microsoft.VisualStudio.Workload.NativeDesktop', 'Microsoft.VisualStudio.Workload.VCTools')) {
        $found = Invoke-BuildCommand $vswhere @('-products', '*', '-requires', $workload, 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath') -Capture
        if (-not [string]::IsNullOrWhiteSpace($found)) { $cppInstallations += $found.Trim() }
    }
    if ($cppInstallations.Count -eq 0) {
        throw 'Visual Studio C++ workload with MSVC x64/x86 tools is missing. Install Desktop development with C++ (or Build Tools C++ workload), including a Windows SDK.'
    }
    Write-BuildLog "C++ installations: $($cppInstallations -join '; ')"

    # Keep the documented output layout even when the caller has Cargo output overrides.
    $env:CARGO_TARGET_DIR = Join-Path $windowsRoot 'target'
    # Explicit host target would add a target-triple subdirectory. Use the validated host.
    Set-BuildEnvironment 'CARGO_BUILD_TARGET' $null
    Invoke-BuildCommand $node @('scripts/check-ui-scripts.mjs')
    foreach ($javascript in @(Get-ChildItem -LiteralPath 'codenotch/ui' -Filter '*.js' -File | Sort-Object Name)) {
        Invoke-BuildCommand $node @('--check', $javascript.FullName)
    }
    $testFiles = @(
        Get-ChildItem -LiteralPath . -Filter 'test-*.cjs' -File
        Get-ChildItem -LiteralPath 'scripts' -Filter 'test-*.cjs' -File
    ) | Sort-Object FullName
    if ($testFiles.Count -eq 0) { throw 'No Node regression tests found in windows/ or windows/scripts/.' }
    # Expand filenames in PowerShell: Windows Node versions differ in glob support.
    Invoke-BuildCommand $node (@('--test') + @($testFiles.FullName))
    Invoke-BuildCommand $cargo @('test', '--locked')

    $previousRustFlags = $env:RUSTFLAGS
    $previousEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
    try {
        $env:RUSTFLAGS = '-C target-feature=+crt-static'
        Set-BuildEnvironment 'CARGO_ENCODED_RUSTFLAGS' $null
        Invoke-BuildCommand $cargo @('build', '--release', '--locked', '-p', 'codenotch-hook', '--target-dir', 'target/hook')
    } finally {
        Set-BuildEnvironment 'RUSTFLAGS' $previousRustFlags
        Set-BuildEnvironment 'CARGO_ENCODED_RUSTFLAGS' $previousEncodedFlags
    }
    $appPath = Join-Path $windowsRoot 'target/release/codenotch.exe'
    $hookPath = Join-Path $windowsRoot 'target/hook/release/codenotch-hook.exe'
    Add-BuildArtifact $hookPath

    if ($Installer) {
        Set-Location -LiteralPath (Join-Path $windowsRoot 'codenotch')
        # Local validation never signs a release or produces an updater feed.
        Set-BuildEnvironment 'TAURI_SIGNING_PRIVATE_KEY' $null
        Set-BuildEnvironment 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD' $null
        Invoke-BuildCommand $npx @('--yes', '@tauri-apps/cli@2.11.4', 'build', '--config', 'tauri.bundle.conf.json', '--bundles', 'nsis', '--', '--locked')
        $version = (Get-Content -LiteralPath 'tauri.conf.json' -Raw | ConvertFrom-Json).version
        $setupPath = Join-Path $windowsRoot "target/release/bundle/nsis/Codenotch_${version}_x64-setup.exe"
        if (-not (Test-Path -LiteralPath $setupPath -PathType Leaf)) { throw "Expected installer is missing: $setupPath" }
        $fixedSetupPath = Join-Path $windowsRoot 'target/local-build/Codenotch-Setup.exe'
        Copy-Item -LiteralPath $setupPath -Destination $fixedSetupPath -Force
        Add-BuildArtifact $fixedSetupPath
    } else {
        # Match Tauri CLI production mode; plain cargo build leaves the context in dev mode.
        Invoke-BuildCommand $cargo @('build', '--release', '--locked', '-p', 'codenotch', '--features', 'tauri/custom-protocol')
    }
    Add-BuildArtifact $appPath
    # The app looks beside itself when the user chooses Install hooks.
    $runtimeHookPath = Join-Path $windowsRoot 'target/release/codenotch-hook.exe'
    Copy-Item -LiteralPath $hookPath -Destination $runtimeHookPath -Force
    Add-BuildArtifact $runtimeHookPath
    $manifest.status = 'passed'
    Write-BuildLog 'Tests and local build passed. Artifacts (SHA256):'
    foreach ($artifact in $manifest.artifacts) { Write-BuildLog "$($artifact.path)  $($artifact.sha256)" }
} catch {
    $manifest.status = 'failed'
    Write-BuildLog "Build stopped: $($_.Exception.Message)"
    throw
} finally {
    Set-Location -LiteralPath $originalLocation.Path
    foreach ($name in $environmentNames) {
        Set-BuildEnvironment $name $originalEnvironment[$name]
    }
    $manifest.finished_at = [DateTimeOffset]::Now.ToString('o')
    $manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding utf8
    Write-Host "Manifest: $manifestPath"
}
