[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'BuildPaths.psm1') -Force
$sfAssertions = 0
function Assert-BuildPath {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
    $script:sfAssertions++
}
$sfRoot = 'C:\Build Root\project'
$sfCargo = Join-Path $sfRoot 'cache\cargo'
$sfRustup = 'D:\Tool Cache\rustup'
$sfBefore = $env:CARGO_ENCODED_RUSTFLAGS
$sfArgs = (Get-SnowRustFlags -Root $sfRoot -CargoHome $sfCargo -RustupHome $sfRustup -Encoded $null -Plain '-C debuginfo=1').Split([char]31)
Assert-BuildPath ($sfArgs[0] -eq '-C') 'Plain flag option was lost'
Assert-BuildPath ($sfArgs[1] -eq 'debuginfo=1') 'Plain flag value was lost'
Assert-BuildPath ($sfArgs.Count -eq 8) 'Mapping arguments were split at spaces'
Assert-BuildPath ($sfArgs[2] -eq "--remap-path-prefix=$sfRoot=/snowflakes") 'Workspace native path missing'
Assert-BuildPath ($sfArgs[3] -eq '--remap-path-prefix=C:/Build Root/project=/snowflakes') 'Workspace slash path missing'
Assert-BuildPath ($sfArgs[4] -eq "--remap-path-prefix=$sfCargo=/cargo") 'Nested Cargo mapping must follow workspace'
Assert-BuildPath ($sfArgs[6] -eq "--remap-path-prefix=$sfRustup=/rustup") 'External Rustup path missing'
$sfExisting = @('--cfg', 'label="two words"', '-C', 'target-feature=+sse2') -join [char]31
$sfEncoded = (Get-SnowRustFlags -Root $sfRoot -CargoHome $sfCargo -RustupHome $sfRustup -Encoded $sfExisting -Plain 'ignored=1').Split([char]31)
Assert-BuildPath ($sfEncoded[0] -eq '--cfg') 'Encoded option changed'
Assert-BuildPath ($sfEncoded[1] -eq 'label="two words"') 'Encoded spaces/quotes changed'
Assert-BuildPath ($sfEncoded[3] -eq 'target-feature=+sse2') 'Existing encoded value changed'
Assert-BuildPath ($sfEncoded -notcontains 'ignored=1') 'Plain flags overrode encoded flags'
Assert-BuildPath ($sfEncoded.Count -eq 10) 'Encoded argument count changed'
$sfEmpty = (Get-SnowRustFlags -Root $sfRoot -CargoHome $sfCargo -RustupHome $sfRustup -Encoded '' -Plain 'ignored=1').Split([char]31)
Assert-BuildPath ($sfEmpty.Count -eq 6) 'Empty encoded flags did not override plain flags'
Assert-BuildPath ($sfEmpty -notcontains 'ignored=1') 'Empty encoded value used plain flags'
Assert-BuildPath ($env:CARGO_ENCODED_RUSTFLAGS -ceq $sfBefore) 'Flag construction changed the environment'
$sfDuplicate = (Get-SnowRustFlags -Root $sfRoot -CargoHome $sfRoot -RustupHome $sfRoot -Encoded $null -Plain '').Split([char]31)
Assert-BuildPath ($sfDuplicate[0].EndsWith('=/snowflakes') -and $sfDuplicate[4].EndsWith('=/rustup')) 'Specific mapping precedence changed'
$sfFixture = Join-Path ([IO.Path]::GetTempPath()) ('snowflakes-build-paths-' + [guid]::NewGuid().ToString('N') + '.bin')
function Assert-RejectedArtifact {
    param([byte[]]$Bytes)
    [IO.File]::WriteAllBytes($sfFixture, $Bytes)
    $sfRejected = $false
    try { $null = & (Join-Path $PSScriptRoot 'check-client-artifact.ps1') -Binary $sfFixture }
    catch { $sfRejected = $_.Exception.Message -match '^Client artifact (contains|is|must)' }
    Assert-BuildPath $sfRejected 'Artifact guard accepted an invalid image or private path'
}
try {
    # Minimal header fixtures test the guard, not a runnable or trusted EXE.
    $sfImage = New-Object byte[] 1024
    [BitConverter]::GetBytes([uint16]0x5a4d).CopyTo($sfImage, 0)
    [BitConverter]::GetBytes([int32]128).CopyTo($sfImage, 0x3c)
    [BitConverter]::GetBytes([uint32]0x4550).CopyTo($sfImage, 128)
    [BitConverter]::GetBytes([uint16]0x8664).CopyTo($sfImage, 132)
    [BitConverter]::GetBytes([uint16]0x20b).CopyTo($sfImage, 152)
    [BitConverter]::GetBytes([uint16]2).CopyTo($sfImage, 220)
    [IO.File]::WriteAllBytes($sfFixture, $sfImage)
    $null = & (Join-Path $PSScriptRoot 'check-client-artifact.ps1') -Binary $sfFixture
    Assert-BuildPath $true 'Neutral header fixture failed'
    $sfActualRoot = Split-Path -Parent $PSScriptRoot
    foreach ($sfCase in @(
        @($sfActualRoot, 'utf8', 300),
        @($sfActualRoot.Replace('\', '/').ToUpperInvariant(), 'utf8', 300),
        @($sfActualRoot, 'utf16', 300),
        @($sfActualRoot, 'utf16', 301),
        @($env:USERPROFILE, 'utf8', 300)
    )) {
        $sfCopy = $sfImage.Clone()
        $sfEncoding = if ($sfCase[1] -eq 'utf8') { [Text.Encoding]::UTF8 } else { [Text.Encoding]::Unicode }
        $sfEncoding.GetBytes($sfCase[0]).CopyTo($sfCopy, $sfCase[2])
        Assert-RejectedArtifact $sfCopy
    }
    foreach ($sfOffset in @(132, 220, 0)) {
        $sfCopy = $sfImage.Clone()
        $sfCopy[$sfOffset] = 0
        Assert-RejectedArtifact $sfCopy
    }
    Assert-RejectedArtifact (New-Object byte[] 10)
    $sfCopy = $sfImage.Clone()
    [BitConverter]::GetBytes([int32]4096).CopyTo($sfCopy, 0x3c)
    Assert-RejectedArtifact $sfCopy
} finally {
    if (Test-Path -LiteralPath $sfFixture) { Remove-Item -LiteralPath $sfFixture }
}
Write-Output "Build paths: $sfAssertions assertions passed"
