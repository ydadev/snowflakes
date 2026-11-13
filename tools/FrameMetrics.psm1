Set-StrictMode -Version Latest

function Get-SnowFrameSnapshot {
    param($Status)
    if ($null -ne $Status -and $Status.PSObject.Properties['frame_metrics']) { $Status.frame_metrics } else { $null }
}
function Get-SnowFramePercentile {
    param($Histogram,$Bounds)
    $count=0L
    foreach ($value in $Histogram) { $count += [long]$value }
    if ($count -eq 0) { return @{upper_ms=$null;overflow=$false} }
    $rank=[long][Math]::Ceiling(0.95 * $count); $sum=0L
    for ($i=0;$i -lt 32;$i++) {
        $sum += [long]$Histogram[$i]
        if ($sum -ge $rank) {
            return @{upper_ms=$(if ($i -lt 31) { [double]$Bounds[$i]/1000.0 } else { $null });overflow=($i -eq 31)}
        }
    }
}
function Assert-SnowFrameInteger {
    param($Value)
    if ($null -eq $Value -or $Value -is [bool]) { throw 'invalid_snapshot' }
    $number=[decimal]$Value
    if ($number -lt 0 -or $number -gt [decimal][long]::MaxValue -or [decimal]::Truncate($number) -ne $number) { throw 'invalid_snapshot' }
}
function Get-SnowFrameDeltaChecked {
    param($Previous,$Current)
    if ($null -eq $Previous -or $null -eq $Current) { return @{state='unavailable';code='not_available';supported=($null -ne $Current);configuration_tracking=$false;configuration_epoch=$null;configuration_changes=0;displays=@()} }
    foreach ($snapshot in @($Previous,$Current)) {
        if ($snapshot.schema_version -notin @(1,2) -or $snapshot.display_latency_measured -ne $false) { throw 'invalid_snapshot' }
        if ($snapshot.schema_version -eq 2) { Assert-SnowFrameInteger $snapshot.configuration_epoch }
        if ($snapshot.omitted_displays -ne 0) { throw 'display_limit' }
        Assert-SnowFrameInteger $snapshot.elapsed_ns
        Assert-SnowFrameInteger $snapshot.sample_age_us
        if ($snapshot.upper_us.Count -ne 31 -or $snapshot.displays.Count -gt 32) { throw 'invalid_snapshot' }
        $last=0L
        foreach ($bound in $snapshot.upper_us) {
            Assert-SnowFrameInteger $bound
            if ($bound -le $last) { throw 'invalid_snapshot' }; $last=[long]$bound
        }
        $ids=@{}
        foreach ($display in $snapshot.displays) {
            if ($display.id -notmatch '^display-[a-zA-Z0-9_-]{1,64}$' -or $ids.ContainsKey($display.id)) { throw 'invalid_snapshot' }
            $ids[$display.id]=$true; Assert-SnowFrameInteger $display.epoch
            if ($display.counters.histogram.Count -ne 32) { throw 'invalid_snapshot' }
            $sum=0L
            foreach ($value in $display.counters.histogram) { Assert-SnowFrameInteger $value; $sum += [long]$value }
            foreach ($key in @('presents','intervals','interval_sum_ns','breaks')) { Assert-SnowFrameInteger $display.counters.$key }
            if ($sum -ne $display.counters.intervals -or $display.counters.intervals -gt $display.counters.presents) { throw 'invalid_snapshot' }
        }
    }
    # An old baseline remains valid historical data. Reject a stale endpoint,
    # then recover the whole accumulated interval once a fresh endpoint arrives.
    if ($Current.sample_age_us -gt 2000000) { throw 'stale_snapshot' }
    if ($Previous.schema_version -ne $Current.schema_version) { throw 'schema_changed' }
    $tracked=$Current.schema_version -eq 2
    $configuration=if ($tracked) { [long]$Current.configuration_epoch } else { $null }
    if ($tracked -and $Current.configuration_epoch -lt $Previous.configuration_epoch) { throw 'invalid_snapshot' }
    if ($tracked -and $Current.configuration_epoch -ne $Previous.configuration_epoch) {
        return @{state='unavailable';code='configuration_changed';supported=$true;configuration_tracking=$true;configuration_epoch=$configuration;configuration_changes=([long]$Current.configuration_epoch-[long]$Previous.configuration_epoch);displays=@()}
    }
    if (($Previous.upper_us -join ',') -ne ($Current.upper_us -join ',')) { throw 'schema_changed' }
    if ($Current.elapsed_ns -le $Previous.elapsed_ns) { throw 'non_advancing_snapshot' }
    $seconds=([double]$Current.elapsed_ns - [double]$Previous.elapsed_ns)/1e9
    $old=@{}; foreach ($entry in $Previous.displays) { $old[$entry.id]=$entry }
    $result=@(); $currentIds=@{}
    foreach ($display in $Current.displays) {
        $currentIds[$display.id]=$true
        $entry=[ordered]@{id=$display.id;epoch=$display.epoch;state='unavailable';code='new_epoch_or_display'}
        if ($old.ContainsKey($display.id) -and $old[$display.id].epoch -eq $display.epoch) {
            $before=$old[$display.id].counters; $after=$display.counters
            $histogram=@(); $valid=$true
            foreach ($key in @('presents','intervals','interval_sum_ns','breaks')) { if ($after.$key -lt $before.$key) { $valid=$false } }
            for ($i=0;$i -lt 32;$i++) { $delta=[long]$after.histogram[$i]-[long]$before.histogram[$i]; if ($delta -lt 0) {$valid=$false}; $histogram += $delta }
            $presents=[long]$after.presents-[long]$before.presents
            $intervals=[long]$after.intervals-[long]$before.intervals
            if ($intervals -gt $presents) { $valid=$false }
            if ($valid) {
                $percentile=Get-SnowFramePercentile $histogram $Current.upper_us
                $entry.state='available';$entry.code='ok'
                $entry.seconds=$seconds;$entry.presents=$presents;$entry.intervals=$intervals
                $entry.breaks=[long]$after.breaks-[long]$before.breaks
                $entry.histogram=$histogram;$entry.upper_us=@($Current.upper_us)
                $entry.present_fps=$presents/$seconds
                $entry.present_interval_p95_upper_ms=$percentile.upper_ms;$entry.p95_overflow=$percentile.overflow
            } else { $entry.code='counters_reset' }
        }
        $result += [PSCustomObject]$entry
    }
    foreach ($entry in $Previous.displays) {
        if (-not $currentIds.ContainsKey($entry.id)) { $result += [PSCustomObject]@{id=$entry.id;epoch=$entry.epoch;state='unavailable';code='removed_display'} }
    }
    @{state=$(if ($result.Count -eq 0) {'unavailable'} elseif (@($result | Where-Object state -eq unavailable).Count -gt 0) {'partial'} else {'available'});code=$(if ($result.Count -eq 0) {'no_displays'} else {'ok'});supported=$true;configuration_tracking=$tracked;configuration_epoch=$configuration;configuration_changes=0;displays=@($result)}
}
function Get-SnowFrameDelta {
    param($Previous,$Current)
    try { Get-SnowFrameDeltaChecked $Previous $Current } catch {
        $code=if ($_.Exception.Message -in @('invalid_snapshot','display_limit','stale_snapshot','schema_changed','non_advancing_snapshot')) {$_.Exception.Message} else {'invalid_snapshot'}
        @{state='unavailable';code=$code;supported=($null -ne $Current);configuration_tracking=$false;configuration_epoch=$null;configuration_changes=0;displays=@()}
    }
}
function New-SnowFrameTotals { @{supported=$false;windows=0;unavailable_windows=0;display_limit=$false;segment_limit=$false;configuration_tracking=$false;legacy_windows=0;configuration_changes=0L;displays=@{}} }
function Add-SnowFrameWindow {
    param($Totals,$Window)
    $Totals.supported=$Totals.supported -or $Window.supported; $Totals.windows++
    $Totals.configuration_tracking=$Totals.configuration_tracking -or $Window.configuration_tracking
    $Totals.configuration_changes += [long]$Window.configuration_changes
    if ($Window.state -in @('available','partial') -and -not $Window.configuration_tracking) { $Totals.legacy_windows++ }
    if ($Window.state -ne 'available') { $Totals.unavailable_windows++ }
    if ($Window.code -eq 'display_limit') { $Totals.display_limit=$true }
    $entryFailure=$false
    foreach ($display in $Window.displays) {
        $key=$(if ($Window.configuration_tracking) { [string]$Window.configuration_epoch } else {'legacy'})+'/'+$display.id
        if (-not $Totals.displays.ContainsKey($key)) {
            if ($Totals.displays.Count -eq 32) { $Totals.display_limit=$true; $Totals.segment_limit=$true; $entryFailure=$true; continue }
            $Totals.displays[$key]=@{id=$display.id;configuration_epoch=$Window.configuration_epoch;seconds=0.0;presents=0L;intervals=0L;breaks=0L;histogram=@(0L)*32;upper_us=$null;valid_windows=0;unavailable_windows=0}
        }
        $entry=$Totals.displays[$key]
        if ($display.state -ne 'available') { $entry.unavailable_windows++; continue }
        if ($null -ne $entry.upper_us -and ($entry.upper_us -join ',') -ne ($display.upper_us -join ',')) { $entry.unavailable_windows++; $entryFailure=$true; continue }
        $entry.upper_us=$display.upper_us
        $entry.seconds += $display.seconds;$entry.presents += $display.presents;$entry.intervals += $display.intervals;$entry.breaks += $display.breaks;$entry.valid_windows++
        for ($i=0;$i -lt 32;$i++) { $entry.histogram[$i]+=$display.histogram[$i] }
    }
    if ($entryFailure -and $Window.state -eq 'available') { $Totals.unavailable_windows++ }
}
function Get-SnowFrameSummary {
    param($Totals)
    $displays=@()
    foreach ($id in @($Totals.displays.Keys | Sort-Object)) {
        $entry=$Totals.displays[$id]
        $p95=Get-SnowFramePercentile $entry.histogram $entry.upper_us
        $displays += [ordered]@{
            id=$entry.id; configuration_epoch=$entry.configuration_epoch; covered_seconds=$entry.seconds; presents=$entry.presents; intervals=$entry.intervals; breaks=$entry.breaks
            valid_windows=$entry.valid_windows;unavailable_windows=$entry.unavailable_windows
            present_fps_mean_when_available=$(if ($entry.seconds -gt 0) {$entry.presents/$entry.seconds} else {$null})
            present_interval_p95_upper_ms=$p95.upper_ms;p95_overflow=$p95.overflow
        }
    }
    @{source='successful_present_calls';supported=$Totals.supported;windows=$Totals.windows;unavailable_windows=$Totals.unavailable_windows;display_limit=$Totals.display_limit;segment_limit=$Totals.segment_limit;configuration_tracking=($Totals.configuration_tracking -and $Totals.legacy_windows -eq 0);legacy_windows=$Totals.legacy_windows;configuration_changes=$Totals.configuration_changes;displays=@($displays);display_latency_measured=$false}
}
Export-ModuleMember -Function Get-SnowFrameSnapshot, Get-SnowFramePercentile, Get-SnowFrameDelta, New-SnowFrameTotals, Add-SnowFrameWindow, Get-SnowFrameSummary
