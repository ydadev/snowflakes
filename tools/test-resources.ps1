[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'ResourceMetrics.psm1') -Force
Add-Type -Path (Join-Path $PSScriptRoot 'ResourceCounters.cs')
$script:checks=0
function Assert-True([bool]$Condition,[string]$Message) { if (-not $Condition) { throw $Message }; $script:checks++ }
function Assert-Fails([scriptblock]$Action,[string]$Code) {
    $caught=$false
    try { & $Action } catch { if ($_.Exception.Message -ne $Code) { throw }; $caught=$true }
    Assert-True $caught "expected_$Code"
}

$cpu=Get-SnowCpuInterval 3.0 3.8 2.0 8
Assert-True ([Math]::Abs($cpu.percent-5.0) -lt 0.000001) 'cpu_not_normalized'
Assert-Fails { Get-SnowCpuInterval 2 1 1 8 } 'invalid_cpu_interval'
Assert-Fails { Get-SnowCpuInterval 0 1 0 8 } 'invalid_cpu_interval'
Assert-Fails { Get-SnowCpuInterval 0 1 1 0 } 'invalid_cpu_interval'
Assert-True ((Get-SnowSampleDelayMilliseconds 0.25 1) -eq 250) 'fractional_remainder_was_truncated'
Assert-True ((Get-SnowSampleDelayMilliseconds 0.0001 1) -eq 1) 'submillisecond_remainder_became_busy_polling'
Assert-True ((Get-SnowSampleDelayMilliseconds 5.0 1) -eq 1000) 'interval_limit_ignored'
Assert-True ((Get-SnowSampleDelayMilliseconds -1.0 1) -eq 0) 'negative_delay'
$counter=[PSCustomObject]@{State='available';Code='ok';Values=@(3.0,7.0)}
Assert-True ((ConvertTo-SnowGpuMetric $counter engine).value -eq 7.0) 'engine_percentages_were_summed'
Assert-True ((ConvertTo-SnowGpuMetric $counter memory).value -eq 10.0) 'adapter_memory_was_not_summed'
$zero=ConvertTo-SnowGpuMetric ([PSCustomObject]@{State='available';Code='ok';Values=@(0.0)}) engine
Assert-True ($zero.state -eq 'available' -and $zero.value -eq 0) 'real_zero_lost'
foreach ($case in @(
    [PSCustomObject]@{State='unavailable';Code='provider_missing';Values=@()},
    [PSCustomObject]@{State='available';Code='ok';Values=@()},
    [PSCustomObject]@{State='available';Code='ok';Values=@([double]::NaN)},
    [PSCustomObject]@{State='available';Code='ok';Values=@(101.0)},
    [PSCustomObject]@{State='available';Code='ok';Values=@(-1.0)}
)) {
    $metric=ConvertTo-SnowGpuMetric $case engine
    Assert-True ($metric.state -eq 'unavailable' -and $null -eq $metric.value) 'unavailable_became_zero'
}

# Use the committed contract fixture; do not depend on a local user's profile.
$settings=Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot '../protocol/fixtures/configuration.json')
$profile=($settings | ConvertFrom-Json).settings
$profile | Add-Member -NotePropertyName server_url -NotePropertyValue 'should_be_excluded'
$profile.snow | Add-Member -NotePropertyName secret -NotePropertyValue 'should_be_excluded'
$state=[PSCustomObject]@{follow_server=$false;source='local';pauses=@('manual');desired_effects_enabled=$true;rendering=$false;settings=$profile;effective_target_fps=60;reduced_detail=$false;server_url='should_be_excluded';hostname='should_be_excluded'}
$json=(Get-SnowProfileSnapshot $state) | ConvertTo-Json -Depth 16 -Compress
Assert-True (-not $json.Contains('should_be_excluded')) 'private_fields_copied'
Assert-True ($json.Contains('flake_size_percent')) 'weather_parameters_missing'
Assert-SnowResourceScenario $state paused
Assert-Fails { Assert-SnowResourceScenario $state snow-baseline } 'scenario_mismatch'
$state.rendering=$true
Assert-Fails { Assert-SnowResourceScenario $state paused } 'scenario_mismatch'
Assert-SnowResourceScenario $state snow-stress

$totals=@{samples=2;seconds=4.0;cpu=0.8;logical=8;cpu_max=10.0;private_first=100;private_last=120;private_max=130;working_max=200;state_changes=1;gpu=@{}}
foreach ($key in @('process_3d','process_committed','dwm_3d')) { $totals.gpu[$key]=@{count=0;seconds=0.0;weighted=0.0;max=$null} }
$totals.gpu.process_3d=@{count=1;seconds=3.0;weighted=18.0;max=6.0}
$summary=New-SnowResourceSummary $totals completed 4.5
Assert-True ([Math]::Abs($summary.cpu_mean_percent-2.5) -lt 0.000001) 'cpu_mean_not_weighted'
Assert-True ($summary.gpu.process_3d.mean_when_available -eq 6) 'gpu_coverage_not_weighted'
Assert-True ($null -eq $summary.gpu.dwm_3d.mean_when_available -and $summary.gpu.dwm_3d.covered_seconds -eq 0) 'missing_gpu_summarized_as_zero'
Assert-True ($summary.private_bytes_growth -eq 20 -and $summary.private_bytes_max -eq 130) 'memory_growth_incorrect'
Assert-True ($null -eq $summary.fps -and $summary.acceptance -eq 'not_evaluated') 'target_fps_became_measurement'

$writer=[IO.StringWriter]::new()
try {
    $sink=@{writer=$writer;bytes=0L;limit=66000L}
    Write-SnowMetricLine $sink @{kind='header';schema_version=1}
    Assert-Fails { Write-SnowMetricLine $sink @{kind='sample';data=('x'*400)} } 'output_limit'
    Write-SnowMetricLine $sink @{kind='summary';reason='output_limit'} -Final
    $lines=$writer.ToString().Trim().Split("`n")
    Assert-True ($lines.Count -eq 2 -and ($lines[1] | ConvertFrom-Json).kind -eq 'summary') 'summary_reserve_lost'
} finally { $writer.Dispose() }
Assert-True ([Snowflakes.Tools.ResourceCounters]::GetActiveProcessorCount(65535) -gt 0) 'native_processor_count_failed'
$native=[Snowflakes.Tools.ResourceCounters]::new($PID,0)
try {
    $native.Collect()
    $missing=$native.Read('not_a_counter')
    Assert-True ($missing.State -eq 'unavailable' -and $missing.Values.Count -eq 0) 'native_missing_counter_not_explicit'
} finally { $native.Dispose() }

# Exercise CLI validation in a separate process: it must not execute a query or
# overwrite the existing output. No app instance is launched by these cases.
$shell=(Get-Process -Id $PID).Path
$tempFile=[IO.Path]::GetTempFileName()
try {
    [IO.File]::WriteAllText($tempFile,'keep')
    $entry=Join-Path $PSScriptRoot 'measure-resources.ps1'
    foreach ($case in @(
        @{output='relative.ndjson';code='absolute_paths_required'},
        @{output='C:relative.ndjson';code='absolute_paths_required'},
        @{output=$tempFile;code='output_already_exists'}
    )) {
        $start=[Diagnostics.ProcessStartInfo]::new()
        $start.FileName=$shell
        $start.Arguments='-NoProfile -NonInteractive -ExecutionPolicy RemoteSigned -File "'+$entry+'" -ClientExe "'+$shell+'" -OutputPath "'+$case.output+'"'
        $start.UseShellExecute=$false; $start.CreateNoWindow=$true
        $start.RedirectStandardOutput=$true; $start.RedirectStandardError=$true
        $child=[Diagnostics.Process]::new(); $child.StartInfo=$start
        try {
            $null=$child.Start(); $stdout=$child.StandardOutput.ReadToEndAsync(); $stderr=$child.StandardError.ReadToEndAsync()
            if (-not $child.WaitForExit(10000)) { $child.Kill(); $child.WaitForExit(); throw 'validation_child_timed_out' }
            $errorText=$stderr.GetAwaiter().GetResult(); $null=$stdout.GetAwaiter().GetResult()
            Assert-True ($child.ExitCode -ne 0 -and $errorText.Contains($case.code)) 'cli_validation_failed'
        } finally { $child.Dispose() }
    }
    Assert-True ([IO.File]::ReadAllText($tempFile) -eq 'keep') 'existing_output_changed'
} finally { [IO.File]::Delete($tempFile) }
Write-Output "Resource collector: $script:checks assertions passed"
