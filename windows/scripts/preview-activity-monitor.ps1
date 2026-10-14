#Requires -Version 7
<#
.SYNOPSIS
Runs the isolated Windows activity-monitor preview on a loopback URL.
.DESCRIPTION
Requires Windows, PowerShell 7 and Node.js 18+. No installation, credentials,
app configuration changes or metric files. The preview prints its URL; Ctrl+C
stops the server and collector. -Check reads a single snapshot without a server.
#>
[CmdletBinding()]
param([ValidateRange(0, 65535)][int]$Port = 0, [switch]$Check)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Este protótipo requer Windows e PowerShell 7.' }
$prototypeRoot = Join-Path $PSScriptRoot '../prototypes/activity-monitor'
if ($Check) {
    & (Join-Path $prototypeRoot 'collector.ps1') -Once
    exit 0
}
$node = Get-Command node -CommandType Application -ErrorAction Stop | Select-Object -First 1
$version = & $node.Source --version
if ([int]($version.TrimStart('v').Split('.')[0]) -lt 18) { throw 'Node.js 18 ou mais recente é necessário.' }
& $node.Source (Join-Path $prototypeRoot 'server.cjs') $Port
exit $LASTEXITCODE
