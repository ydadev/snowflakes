[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
Import-Module (Join-Path $PSScriptRoot 'FrameMetrics.psm1') -Force
$script:checks=0
function Assert-Frame([bool]$Condition,[string]$Message) { if (-not $Condition) {throw $Message}; $script:checks++ }
$bounds=@(1000,2000,4000,8000,10000,12000,14000,16000,17000,18000,19000,20000,21000,22000,23000,24000,25000,26000,28000,30000,33000,34000,40000,50000,66000,100000,200000,500000,1000000,5000000,10000000)
function New-FrameDisplay([string]$Id,[long]$Intervals,[int]$Bucket=8,[long]$Epoch=1) {
    $histogram=@(0L)*32;$histogram[$Bucket]=$Intervals
    [PSCustomObject]@{id=$Id;epoch=$Epoch;counters=[PSCustomObject]@{presents=$Intervals+1;intervals=$Intervals;interval_sum_ns=$Intervals*10000000L;histogram=$histogram;breaks=0;continuous=$true}}
}
function New-FrameSnapshot([long]$Ns,$Displays) {
    [PSCustomObject]@{schema_version=1;elapsed_ns=$Ns;sample_age_us=0;display_latency_measured=$false;upper_us=@($bounds);omitted_displays=0;displays=@($Displays)}
}
$previous=New-FrameSnapshot 0 @((New-FrameDisplay display-a 0),(New-FrameDisplay display-b 0))
$current=New-FrameSnapshot 2000000000 @((New-FrameDisplay display-a 120),(New-FrameDisplay display-b 60 21))
$delta=Get-SnowFrameDelta $previous $current
Assert-Frame ($delta.state -eq 'available') 'valid_pair_unavailable'
Assert-Frame ($delta.displays[0].present_fps -eq 60 -and $delta.displays[1].present_fps -eq 30) 'display_counts_combined'
Assert-Frame ($delta.displays[0].present_interval_p95_upper_ms -eq 17 -and $delta.displays[1].present_interval_p95_upper_ms -eq 34) 'histogram_bounds_incorrect'
$paused=New-FrameSnapshot 4000000000 @((New-FrameDisplay display-a 120),(New-FrameDisplay display-b 60 21))
$zero=Get-SnowFrameDelta $current $paused
Assert-Frame ($zero.displays[0].present_fps -eq 0 -and $null -eq $zero.displays[0].present_interval_p95_upper_ms) 'pause_invented_intervals'
$reset=New-FrameSnapshot 4000000000 @((New-FrameDisplay display-a 1 8 2))
$resetResult=Get-SnowFrameDelta $current $reset
Assert-Frame ($resetResult.state -eq 'partial' -and $resetResult.displays[0].code -eq 'new_epoch_or_display' -and $resetResult.displays[1].code -eq 'removed_display') 'epochs_or_removed_displays_hidden'
$current.sample_age_us=2000001
Assert-Frame ((Get-SnowFrameDelta $previous $current).code -eq 'stale_snapshot') 'stale_data_accepted'
$current.sample_age_us=0;$current.omitted_displays=1
Assert-Frame ((Get-SnowFrameDelta $previous $current).code -eq 'display_limit') 'truncated_data_accepted'
$truncated=New-SnowFrameTotals
Add-SnowFrameWindow $truncated (Get-SnowFrameDelta $previous $current)
Assert-Frame ((Get-SnowFrameSummary $truncated).display_limit) 'source_display_limit_missing_in_summary'
$current.omitted_displays=0;$current.displays[0].counters.histogram[0]=-1
Assert-Frame ((Get-SnowFrameDelta $previous $current).code -eq 'invalid_snapshot') 'negative_bin_accepted'
$current.displays[0].counters.histogram[0]=0;$current.displays[0].counters.presents=1
Assert-Frame ((Get-SnowFrameDelta $previous $current).code -eq 'invalid_snapshot') 'impossible_counts_accepted'
Assert-Frame ((Get-SnowFrameDelta $previous $previous).code -eq 'non_advancing_snapshot') 'non_advancing_data_accepted'
Assert-Frame ((Get-SnowFrameDelta $null $null).code -eq 'not_available') 'legacy_client_became_zero_fps'
Assert-Frame ($null -eq (Get-SnowFrameSnapshot ([PSCustomObject]@{rendering=$false}))) 'missing_optional_field_failed'
$invalid=New-FrameSnapshot 2000000000 @((New-FrameDisplay display-a 120))
$invalid.displays[0].counters.presents=$null
Assert-Frame ((Get-SnowFrameDelta $previous $invalid).code -eq 'invalid_snapshot') 'null_counter_became_zero'
$invalid=New-FrameSnapshot 2000000000 @((New-FrameDisplay display-a 120))
$invalid.upper_us[0]=999
Assert-Frame ((Get-SnowFrameDelta $previous $invalid).code -eq 'schema_changed') 'different_bounds_combined'
$invalid=New-FrameSnapshot 2000000000 @((New-FrameDisplay display-a 120),(New-FrameDisplay display-a 1))
Assert-Frame ((Get-SnowFrameDelta $previous $invalid).code -eq 'invalid_snapshot') 'duplicate_display_accepted'
$earlier=New-FrameSnapshot 1000000000 @((New-FrameDisplay display-a 120))
$later=New-FrameSnapshot 2000000000 @((New-FrameDisplay display-a 110))
Assert-Frame ((Get-SnowFrameDelta $earlier $later).displays[0].code -eq 'counters_reset') 'decreased_counters_accepted'
$stalled=New-FrameSnapshot 0 @((New-FrameDisplay display-a 0))
$stalled.sample_age_us=3000000
$recovered=New-FrameSnapshot 30000000000 @((New-FrameDisplay display-a 1 31))
$recovered.displays[0].counters.interval_sum_ns=30000000000L
$recoveredWindow=Get-SnowFrameDelta $stalled $recovered
Assert-Frame ($recoveredWindow.state -eq 'available' -and $recoveredWindow.displays[0].p95_overflow -and $recoveredWindow.displays[0].seconds -eq 30) 'recovered_stall_disappeared_from_distribution'

# Aggregate distributions, not window p95 values or arithmetic means of FPS.
$start=New-FrameSnapshot 0 @((New-FrameDisplay display-a 0))
$middle=New-FrameSnapshot 1500000000 @((New-FrameDisplay display-a 90 5))
$end=New-FrameSnapshot 2500000000 @((New-FrameDisplay display-a 100 5))
$end.displays[0].counters.histogram[5]=90;$end.displays[0].counters.histogram[19]=10
$totals=New-SnowFrameTotals
Add-SnowFrameWindow $totals (Get-SnowFrameDelta $start $middle)
Add-SnowFrameWindow $totals (Get-SnowFrameDelta $middle $end)
$summary=Get-SnowFrameSummary $totals
Assert-Frame ($summary.displays[0].present_fps_mean_when_available -eq 40) 'fps_not_weighted_by_time'
Assert-Frame ($summary.displays[0].present_interval_p95_upper_ms -eq 30) 'window_percentiles_were_averaged'
$histogram=@(0L)*32;$histogram[16]=95;$histogram[31]=5
Assert-Frame ((Get-SnowFramePercentile $histogram $bounds).upper_ms -eq 25) 'exact_95_percent_boundary_failed'
$histogram[16]=94;$histogram[31]=6
$overflow=Get-SnowFramePercentile $histogram $bounds
Assert-Frame ($overflow.overflow -and $null -eq $overflow.upper_ms) 'long_stalls_were_capped'
$window=Get-SnowFrameDelta $start $middle
for ($i=0;$i -lt 34;$i++) { $window.displays[0].id="display-$i"; Add-SnowFrameWindow $totals $window }
Assert-Frame ($totals.displays.Count -eq 32 -and (Get-SnowFrameSummary $totals).display_limit) 'historical_display_set_unbounded'

function New-TrackedSnapshot([long]$Ns,[long]$Configuration,$Displays) {
    $snapshot=New-FrameSnapshot $Ns $Displays
    $snapshot.schema_version=2
    $snapshot | Add-Member -NotePropertyName configuration_epoch -NotePropertyValue $Configuration
    $snapshot
}
$a0=New-TrackedSnapshot 0 1 @((New-FrameDisplay display-a 0))
$a1=New-TrackedSnapshot 1500000000 1 @((New-FrameDisplay display-a 90 5))
$aRestored=New-TrackedSnapshot 2500000000 3 @((New-FrameDisplay display-a 100 5))
$aLater=New-TrackedSnapshot 3500000000 3 @((New-FrameDisplay display-a 130 5))
$aLater.displays[0].counters.histogram[5]=100;$aLater.displays[0].counters.histogram[21]=30
$crossing=Get-SnowFrameDelta $a1 $aRestored
Assert-Frame ($crossing.code -eq 'configuration_changed' -and $crossing.configuration_changes -eq 2 -and $crossing.displays.Count -eq 0) 'a_b_a_was_hidden_by_polling'
$trackedTotals=New-SnowFrameTotals
Add-SnowFrameWindow $trackedTotals (Get-SnowFrameDelta $a0 $a1)
Add-SnowFrameWindow $trackedTotals $crossing
Add-SnowFrameWindow $trackedTotals (Get-SnowFrameDelta $aRestored $aLater)
$trackedSummary=Get-SnowFrameSummary $trackedTotals
Assert-Frame ($trackedSummary.configuration_tracking -and $trackedSummary.configuration_changes -eq 2) 'configuration_changes_missing_in_summary'
Assert-Frame ($trackedSummary.displays.Count -eq 2 -and $trackedSummary.displays[0].configuration_epoch -eq 1 -and $trackedSummary.displays[1].configuration_epoch -eq 3 -and $trackedSummary.displays[0].present_fps_mean_when_available -eq 60 -and $trackedSummary.displays[1].present_fps_mean_when_available -eq 30) 'configuration_segments_were_merged'
Assert-Frame ($trackedSummary.displays[0].present_interval_p95_upper_ms -eq 12 -and $trackedSummary.displays[1].present_interval_p95_upper_ms -eq 34) 'configuration_distributions_were_merged'
$legacyTotals=New-SnowFrameTotals
Add-SnowFrameWindow $legacyTotals (Get-SnowFrameDelta $start $middle)
Assert-Frame (-not (Get-SnowFrameSummary $legacyTotals).configuration_tracking -and $legacyTotals.legacy_windows -eq 1) 'legacy_was_claimed_as_tracked'
Assert-Frame ((Get-SnowFrameDelta $aRestored $a1).code -eq 'invalid_snapshot') 'configuration_epoch_rollback_accepted'
$missing=New-FrameSnapshot 2000000000 @((New-FrameDisplay display-a 100));$missing.schema_version=2
Assert-Frame ((Get-SnowFrameDelta $a0 $missing).code -eq 'invalid_snapshot') 'missing_configuration_epoch_accepted'
$segmentTotals=New-SnowFrameTotals;$window=Get-SnowFrameDelta $a0 $a1
for ($i=1;$i -le 34;$i++) { $window.configuration_epoch=$i;Add-SnowFrameWindow $segmentTotals $window }
Assert-Frame ($segmentTotals.displays.Count -eq 32 -and (Get-SnowFrameSummary $segmentTotals).segment_limit) 'configuration_segments_unbounded'
Write-Output "Frame metrics: $script:checks assertions passed"
