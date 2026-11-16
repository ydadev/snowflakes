Set-StrictMode -Version Latest

function Get-SnowRustFlags {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Root,
        [string]$CargoHome = $env:CARGO_HOME,
        [string]$RustupHome = $env:RUSTUP_HOME,
        [AllowNull()][object]$Encoded = $env:CARGO_ENCODED_RUSTFLAGS,
        [AllowNull()][string]$Plain = $env:RUSTFLAGS
    )
    $sfSeparator = [char]31
    $sfFlags = [Collections.Generic.List[string]]::new()
    if ($null -ne $Encoded) {
        if ([string]$Encoded -ne '') {
            foreach ($sfFlag in ([string]$Encoded).Split($sfSeparator)) { $sfFlags.Add($sfFlag) }
        }
    } elseif (-not [string]::IsNullOrWhiteSpace($Plain)) {
        foreach ($sfFlag in [regex]::Split($Plain.Trim(), '\s+')) { $sfFlags.Add($sfFlag) }
    }
    if (-not $CargoHome) { $CargoHome = Join-Path $env:USERPROFILE '.cargo' }
    if (-not $RustupHome) { $RustupHome = Join-Path $env:USERPROFILE '.rustup' }
    # General mapping first; narrower Cargo/Rustup mappings win when nested.
    foreach ($sfMapping in @(
        @($Root, '/snowflakes'), @($CargoHome, '/cargo'), @($RustupHome, '/rustup')
    )) {
        $sfPath = [IO.Path]::GetFullPath($sfMapping[0]).TrimEnd([char[]]'\/')
        foreach ($sfVariant in @($sfPath, $sfPath.Replace('\', '/')) | Select-Object -Unique) {
            $sfFlags.Add("--remap-path-prefix=$sfVariant=$($sfMapping[1])")
        }
    }
    return [string]::Join($sfSeparator, $sfFlags)
}

Export-ModuleMember -Function Get-SnowRustFlags
