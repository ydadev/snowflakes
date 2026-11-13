[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$ClientExe,
    [Parameter(Mandatory)][string]$OutputPath,
    [ValidateRange(1,28800)][int]$Seconds=60,
    [ValidateRange(1,10)][int]$IntervalSeconds=1,
    [ValidateRange(0,60)][int]$WarmupSeconds=5,
    [ValidateSet('paused','snow-baseline','snow-stress')][string]$Scenario='paused'
)
$ErrorActionPreference='Stop'
try {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT -or -not [Environment]::Is64BitProcess) { throw 'windows_x64_required' }
    foreach ($path in @($ClientExe,$OutputPath)) {
        if ($path -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$))') { throw 'absolute_paths_required' }
    }
    if (-not (Test-Path -LiteralPath $ClientExe -PathType Leaf)) { throw 'client_executable_missing' }
    if (Test-Path -LiteralPath $OutputPath) { throw 'output_already_exists' }
    Add-Type -Path (Join-Path $PSScriptRoot 'ResourceCounters.cs')
    Import-Module (Join-Path $PSScriptRoot 'ResourceMetrics.psm1') -Force
    $summary=Invoke-SnowResourceCollection -ClientExe $ClientExe -OutputPath $OutputPath -Seconds $Seconds -IntervalSeconds $IntervalSeconds -WarmupSeconds $WarmupSeconds -Scenario $Scenario
    $summary | ConvertTo-Json -Compress -Depth 8
    if ($summary.reason -ne 'completed') { exit 1 }
} catch {
    $code = if ($_.Exception.Message -in @('windows_x64_required','absolute_paths_required','client_executable_missing','output_already_exists','client_query_failed','client_query_timeout','invalid_client_status','target_changed','processor_count_unavailable','scenario_mismatch')) { $_.Exception.Message } else { 'resource_collection_failed' }
    Write-Error $code
    exit 1
}
