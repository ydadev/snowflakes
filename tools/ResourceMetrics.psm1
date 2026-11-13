Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'FrameMetrics.psm1') -Force

function Get-SnowCpuInterval {
    param([double]$PreviousCpuSeconds, [double]$CpuSeconds, [double]$Seconds, [int]$LogicalProcessors)
    if ($LogicalProcessors -le 0 -or $Seconds -le 0 -or $CpuSeconds -lt $PreviousCpuSeconds) {
        throw 'invalid_cpu_interval'
    }
    $delta = $CpuSeconds - $PreviousCpuSeconds
    [PSCustomObject]@{ seconds=$Seconds; cpu_seconds=$delta; percent=100.0 * $delta / $Seconds / $LogicalProcessors }
}

function Get-SnowSampleDelayMilliseconds {
    param([double]$RemainingSeconds, [int]$IntervalSeconds)
    # Both arguments must be double. PowerShell otherwise selects Math.Min(int,
    # int) and truncates the last fractional second into an active polling loop.
    $delay = [Math]::Max(0.0, [Math]::Min([double]$IntervalSeconds, $RemainingSeconds))
    [int][Math]::Ceiling(1000.0 * $delay)
}

function ConvertTo-SnowGpuMetric {
    param($Counter, [ValidateSet('engine','memory')][string]$Kind)
    if ($Counter.State -ne 'available' -or $Counter.Values.Count -eq 0) {
        return [PSCustomObject]@{ state='unavailable'; code=$Counter.Code; value=$null; instances=0 }
    }
    $value = 0.0
    foreach ($number in $Counter.Values) {
        if ([double]::IsNaN($number) -or [double]::IsInfinity($number) -or $number -lt 0 -or ($Kind -eq 'engine' -and $number -gt 100)) {
            return [PSCustomObject]@{ state='unavailable'; code='invalid_value'; value=$null; instances=0 }
        }
        if ($Kind -eq 'engine') { $value = [Math]::Max($value, $number) } else { $value += $number }
    }
    [PSCustomObject]@{ state='available'; code='ok'; value=$value; instances=$Counter.Values.Count }
}

function Read-SnowClientJson {
    param([string]$ClientExe, [ValidateSet('status','monitors')][string]$Command = 'status')
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $ClientExe
    $start.Arguments = if ($Command -eq 'status') { '--status --json' } else { '--list-monitors --json' }
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $child = [Diagnostics.Process]::new()
    $child.StartInfo = $start
    try {
        if (-not $child.Start()) { throw 'client_query_failed' }
        $stdout = $child.StandardOutput.ReadToEndAsync()
        $stderr = $child.StandardError.ReadToEndAsync()
        if (-not $child.WaitForExit(5000)) {
            $child.Kill() # Only the query child started above, never the target.
            $child.WaitForExit()
            throw 'client_query_timeout'
        }
        $text = $stdout.GetAwaiter().GetResult()
        $null = $stderr.GetAwaiter().GetResult()
        if ($child.ExitCode -ne 0 -or $text.Length -gt 65536) { throw 'client_query_failed' }
        $result = $text | ConvertFrom-Json -ErrorAction Stop
        if ($result.code -ne 0) { throw 'client_query_failed' }
        $result
    } finally { $child.Dispose() }
}

function Get-SnowProfileSnapshot {
    param($Status)
    # Explicit allowlist: no endpoint, paths, title, hostname, SID or network fields.
    $settings = $null
    if ($null -ne $Status.settings) {
        $settings = [ordered]@{ effects_enabled=$Status.settings.effects_enabled }
        $fields = [ordered]@{
            weather=@('mode','opacity_percent')
            snow=@('intensity_percent','flake_size_percent','flake_visibility_percent','drift_size_percent','wind_percent','on_windows','on_ground','melting')
            rain=@('intensity_percent','puddle_size_percent','wind_percent','on_windows','on_ground','evaporation')
            interaction=@('cursor_enabled')
            performance=@('target_fps','battery_saver','pause_fullscreen','pause_remote_session')
            display=@('selection','monitor_ids')
        }
        foreach ($group in $fields.Keys) {
            $settings[$group]=[ordered]@{}
            foreach ($key in $fields[$group]) { $settings[$group][$key]=$Status.settings.$group.$key }
        }
    }
    [ordered]@{
        follow_server=$Status.follow_server; source=$Status.source
        pauses=@($Status.pauses); desired_effects_enabled=$Status.desired_effects_enabled
        rendering=$Status.rendering; settings=$settings
        effective_target_fps=$Status.effective_target_fps; reduced_detail=$Status.reduced_detail
    }
}

function Get-SnowProcessPoint {
    param([Diagnostics.Process]$Process, [Diagnostics.Stopwatch]$Clock, [long]$ExpectedStart)
    $Process.Refresh()
    if ($Process.HasExited -or $Process.StartTime.ToUniversalTime().Ticks -ne $ExpectedStart) { throw 'target_exited' }
    [PSCustomObject]@{
        seconds=$Clock.Elapsed.TotalSeconds; cpu_seconds=$Process.TotalProcessorTime.TotalSeconds
        private_bytes=$Process.PrivateMemorySize64; working_set_bytes=$Process.WorkingSet64
    }
}

function Assert-SnowResourceScenario {
    param($Status, [string]$Scenario)
    if ($Scenario -eq 'paused') {
        if ($Status.rendering) { throw 'scenario_mismatch' }
    } elseif (-not $Status.rendering -or $null -eq $Status.settings -or $Status.settings.weather.mode -ne 'snow') {
        throw 'scenario_mismatch'
    }
}

function Write-SnowMetricLine {
    param($Sink, $Record, [switch]$Final)
    $line = $Record | ConvertTo-Json -Compress -Depth 16
    $bytes = [Text.Encoding]::UTF8.GetByteCount($line) + 1
    $reserve = if ($Final) { 0 } else { 65536 }
    if ($Sink.bytes + $bytes + $reserve -gt $Sink.limit) { throw 'output_limit' }
    $Sink.writer.WriteLine($line)
    $Sink.bytes += $bytes
}

function New-SnowResourceSummary {
    param($Totals, [string]$Reason, [double]$ElapsedSeconds)
    $frames=if ($Totals.ContainsKey('frames')) { Get-SnowFrameSummary $Totals.frames } else { Get-SnowFrameSummary (New-SnowFrameTotals) }
    $gpu = [ordered]@{}
    foreach ($key in @('process_3d','process_committed','dwm_3d')) {
        $entry = $Totals.gpu[$key]
        $mean = if ($entry.seconds -gt 0) { $entry.weighted / $entry.seconds } else { $null }
        $gpu[$key] = @{ valid_samples=$entry.count; covered_seconds=$entry.seconds; mean_when_available=$mean; max=$entry.max }
    }
    [ordered]@{
        kind='summary'; reason=$Reason; elapsed_seconds=$ElapsedSeconds; samples=$Totals.samples
        cpu_mean_percent=$(if ($Totals.seconds -gt 0) { 100.0 * $Totals.cpu / $Totals.seconds / $Totals.logical } else { $null })
        cpu_max_percent=$Totals.cpu_max; cpu_covered_seconds=$Totals.seconds
        private_bytes_max=$Totals.private_max; private_bytes_growth=$Totals.private_last - $Totals.private_first
        working_set_bytes_max=$Totals.working_max; gpu=$gpu; state_changes=$Totals.state_changes
        fps=$null; frame_interval_p95_ms=$null; frame_telemetry=$(if ($frames.supported) {'successful_present_calls'} else {'not_available'}); frames=$frames; acceptance='not_evaluated'
    }
}

function Get-SnowHardware {
    param([string]$ClientExe, [int]$LogicalProcessors)
    $cpu = Get-ItemPropertyValue -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\HARDWARE\DESCRIPTION\System\CentralProcessor\0' -Name ProcessorNameString -ErrorAction SilentlyContinue
    $os = Get-ItemProperty -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    $adapters = @(Get-ChildItem -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\Video' -ErrorAction SilentlyContinue | ForEach-Object {
        $description = Get-ItemProperty -LiteralPath (Join-Path $_.PSPath '0000') -ErrorAction SilentlyContinue
        if ($null -ne $description -and $description.PSObject.Properties['DriverDesc']) {
            [ordered]@{ name=$description.DriverDesc; driver=$(if ($description.PSObject.Properties['DriverVersion']) { $description.DriverVersion } else { $null }) }
        }
    })
    $monitors = $null
    try { $monitors = @((Read-SnowClientJson $ClientExe monitors).monitors | ForEach-Object {
        [ordered]@{left=$_.left;top=$_.top;width=$_.width;height=$_.height;primary=$_.primary}
    }) } catch { }
    [ordered]@{
        cpu=$cpu; logical_processors=$LogicalProcessors; physical_memory_bytes=[Snowflakes.Tools.ResourceCounters]::PhysicalMemoryBytes()
        os_product=$os.ProductName; os_build=$os.CurrentBuildNumber; os_revision=$os.UBR
        process_architecture=$env:PROCESSOR_ARCHITECTURE; gpu_adapters=$adapters; monitors=$monitors
    }
}

function Invoke-SnowResourceCollection {
    param([string]$ClientExe, [string]$OutputPath, [int]$Seconds, [int]$IntervalSeconds, [int]$WarmupSeconds, [string]$Scenario)
    $initial = (Read-SnowClientJson $ClientExe).status
    if ($null -eq $initial -or $initial.pid -le 0) { throw 'invalid_client_status' }
    $process = Get-Process -Id $initial.pid -ErrorAction Stop
    if ($process.SessionId -ne $initial.session_id) { $process.Dispose(); throw 'target_changed' }
    $startTicks = $process.StartTime.ToUniversalTime().Ticks
    $targetHash = $null
    try { $targetHash = (Get-FileHash -LiteralPath $process.MainModule.FileName -Algorithm SHA256).Hash } catch { }
    $queryHash = (Get-FileHash -LiteralPath $ClientExe -Algorithm SHA256).Hash
    $dwm = @(Get-Process -Name dwm -ErrorAction SilentlyContinue | Where-Object SessionId -eq $initial.session_id)
    $dwmProcess = if ($dwm.Count -eq 1) { $dwm[0] } else { $null }
    $dwmId = if ($dwmProcess) { $dwmProcess.Id } else { 0 }
    $logical = [Snowflakes.Tools.ResourceCounters]::GetActiveProcessorCount(65535)
    if ($logical -eq 0) { throw 'processor_count_unavailable' }
    $file = $null; $writer = $null; $counters = $null; $summary = $null
    try {
        $file = [IO.FileStream]::new($OutputPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        $writer = [IO.StreamWriter]::new($file, [Text.UTF8Encoding]::new($false))
        $writer.NewLine = "`n"
        $sink = @{ writer=$writer; bytes=0L; limit=64L * 1024 * 1024 }
        $counters = [Snowflakes.Tools.ResourceCounters]::new($initial.pid, $dwmId)
        $counters.Collect()
        $hardware = Get-SnowHardware $ClientExe $logical
        if ($WarmupSeconds -gt 0) { Start-Sleep -Seconds $WarmupSeconds }
        $verified = (Read-SnowClientJson $ClientExe).status
        if ($verified.pid -ne $initial.pid) { throw 'target_changed' }
        Assert-SnowResourceScenario $verified $Scenario
        $snapshot = Get-SnowProfileSnapshot $verified
        $fingerprint = $snapshot | ConvertTo-Json -Compress -Depth 16
        $previousFrames = Get-SnowFrameSnapshot $verified
        $frameSupported=$null -ne $previousFrames
        if ($frameSupported -and $previousFrames.sample_age_us -gt 2000000) { $previousFrames=$null }
        Write-SnowMetricLine $sink ([ordered]@{
            kind='header'; schema_version=2; client_version=$verified.version; process_id=$verified.pid
            target_executable_sha256=$targetHash; query_executable_sha256=$queryHash
            scenario=$Scenario; requested_seconds=$Seconds; interval_seconds=$IntervalSeconds; warmup_seconds=$WarmupSeconds
            hardware=$hardware; state=$snapshot; dwm_process_id=$dwmId; gpu_source='pdh_english_counters'; gpu_memory_validation='provider_value_requires_cross_check'
            gpu_3d_aggregation='busiest_3d_engine'; fps=$null; frame_telemetry=$(if ($frameSupported) {'successful_present_calls'} else {'not_available'}); frame_baseline_available=($null -ne $previousFrames); display_latency_measured=$false
        })
        $counters.Collect() # End the warm-up interval before measuring GPU usage.
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $previous = Get-SnowProcessPoint $process $clock $startTicks
        $deadline = $previous.seconds + $Seconds
        $totals = @{ logical=$logical; samples=0; seconds=0.0; cpu=0.0; cpu_max=0.0; private_first=$previous.private_bytes; private_last=$previous.private_bytes; private_max=$previous.private_bytes; working_max=$previous.working_set_bytes; state_changes=0; gpu=@{};frames=(New-SnowFrameTotals) }
        $totals.frames.supported=$frameSupported
        foreach ($key in @('process_3d','process_committed','dwm_3d')) { $totals.gpu[$key]=@{ count=0; seconds=0.0; weighted=0.0; max=$null } }
        $reason = 'completed'; $nextStatus = 5.0
        try {
            while ($clock.Elapsed.TotalSeconds -lt $deadline) {
                $delay = Get-SnowSampleDelayMilliseconds ($deadline - $clock.Elapsed.TotalSeconds) $IntervalSeconds
                if ($delay -gt 0) { Start-Sleep -Milliseconds $delay }
                $counters.Collect()
                $point = Get-SnowProcessPoint $process $clock $startTicks
                $cpu = Get-SnowCpuInterval $previous.cpu_seconds $point.cpu_seconds ($point.seconds - $previous.seconds) $logical
                $gpu = [ordered]@{}
                foreach ($key in @('process_3d','process_committed','dwm_3d')) {
                    $kind = if ($key -eq 'process_committed') { 'memory' } else { 'engine' }
                    $gpu[$key] = ConvertTo-SnowGpuMetric ($counters.Read($key)) $kind
                }
                $currentDwm = @(Get-Process -Name dwm -ErrorAction SilentlyContinue | Where-Object SessionId -eq $initial.session_id)
                $dwmStable = $dwmId -gt 0 -and $currentDwm.Count -eq 1 -and $currentDwm[0].Id -eq $dwmId
                foreach ($entry in $currentDwm) { $entry.Dispose() }
                if (-not $dwmStable) { $gpu.dwm_3d=[PSCustomObject]@{state='unavailable';code='dwm_not_stable';value=$null;instances=0} }
                Write-SnowMetricLine $sink ([ordered]@{
                    kind='sample'; elapsed_seconds=$point.seconds; interval_seconds=$cpu.seconds; state_revision=$totals.state_changes
                    cpu_percent=$cpu.percent; private_bytes=$point.private_bytes; working_set_bytes=$point.working_set_bytes
                    gpu=$gpu
                })
                $totals.samples++; $totals.seconds += $cpu.seconds; $totals.cpu += $cpu.cpu_seconds
                $totals.cpu_max=[Math]::Max($totals.cpu_max,$cpu.percent)
                $totals.private_last=$point.private_bytes; $totals.private_max=[Math]::Max($totals.private_max,$point.private_bytes)
                $totals.working_max=[Math]::Max($totals.working_max,$point.working_set_bytes)
                foreach ($key in @('process_3d','process_committed','dwm_3d')) {
                    if ($gpu[$key].state -eq 'available') {
                        $entry=$totals.gpu[$key]; $entry.count++; $entry.seconds += $cpu.seconds; $entry.weighted += $gpu[$key].value * $cpu.seconds
                        $entry.max=$(if ($null -eq $entry.max) { $gpu[$key].value } else { [Math]::Max($entry.max,$gpu[$key].value) })
                    }
                }
                $previous = $point
                if ($point.seconds -ge $nextStatus -or $point.seconds -ge $deadline) {
                    $state = (Read-SnowClientJson $ClientExe).status
                    if ($state.pid -ne $initial.pid) { throw 'target_changed' }
                    $currentFrames = Get-SnowFrameSnapshot $state
                    if ($null -ne $previousFrames -or $null -ne $currentFrames) {
                        $frameWindow=Get-SnowFrameDelta $previousFrames $currentFrames
                        Add-SnowFrameWindow $totals.frames $frameWindow
                        Write-SnowMetricLine $sink @{kind='frames';elapsed_seconds=$clock.Elapsed.TotalSeconds;window=$frameWindow}
                    }
                    # Before the first fresh baseline, do not import pre-capture
                    # time from an already stalled renderer into this window.
                    if ($null -ne $previousFrames -or ($null -ne $currentFrames -and $currentFrames.sample_age_us -le 2000000)) { $previousFrames=$currentFrames }
                    $current = Get-SnowProfileSnapshot $state
                    $currentFingerprint = $current | ConvertTo-Json -Compress -Depth 16
                    if ($fingerprint -ne $currentFingerprint) {
                        $totals.state_changes++; $fingerprint=$currentFingerprint
                        Write-SnowMetricLine $sink @{kind='state';elapsed_seconds=$clock.Elapsed.TotalSeconds;revision=$totals.state_changes;state=$current}
                    }
                    $nextStatus = $clock.Elapsed.TotalSeconds + 5
                }
            }
        } catch {
            $reason = if ($_.Exception.Message -in @('target_exited','target_changed','output_limit','invalid_cpu_interval','client_query_timeout','client_query_failed')) { $_.Exception.Message } else { 'collection_failed' }
        }
        $summary = New-SnowResourceSummary $totals $reason $clock.Elapsed.TotalSeconds
        Write-SnowMetricLine $sink $summary -Final
        $writer.Flush()
    } finally {
        if ($counters) { $counters.Dispose() }
        if ($writer) { $writer.Dispose() } elseif ($file) { $file.Dispose() }
        $process.Dispose()
        foreach ($entry in $dwm) { $entry.Dispose() }
    }
    $summary
}

Export-ModuleMember -Function Get-SnowCpuInterval, Get-SnowSampleDelayMilliseconds, ConvertTo-SnowGpuMetric, Get-SnowProfileSnapshot, Write-SnowMetricLine, New-SnowResourceSummary, Read-SnowClientJson, Invoke-SnowResourceCollection, Assert-SnowResourceScenario
