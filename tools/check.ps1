[CmdletBinding()]
param(
    [string]$Php = '',
    [switch]$SkipHttp
)

$ErrorActionPreference = 'Stop'
$sfRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $sfRoot
try {
    $sfCargoHome = Join-Path $sfRoot '.local/tools/cargo'
    $sfRustupHome = Join-Path $sfRoot '.local/tools/rustup'
    if (Test-Path -LiteralPath (Join-Path $sfCargoHome 'bin/cargo.exe')) {
        $env:CARGO_HOME = $sfCargoHome
        $env:RUSTUP_HOME = $sfRustupHome
        $env:PATH = (Join-Path $sfCargoHome 'bin') + [IO.Path]::PathSeparator + $env:PATH
    }
    if (-not $Php) {
        $sfPortablePhp = Join-Path $sfRoot '.local/tools/php/php.exe'
        $Php = if (Test-Path -LiteralPath $sfPortablePhp) { $sfPortablePhp } else { 'php' }
    }

    function Invoke-Checked {
        param([scriptblock]$Command)
        & $Command
        if ($LASTEXITCODE -ne 0) { throw "Check failed (exit $LASTEXITCODE)" }
    }

    $env:CARGO_INCREMENTAL = '0'
    Invoke-Checked { cargo fmt --all --check }
    Invoke-Checked { cargo clippy --workspace --all-targets --locked -- -D warnings }
    Invoke-Checked { cargo test --workspace --locked }
    Invoke-Checked { & .\tools\test-resources.ps1 }
    Invoke-Checked { & .\tools\test-frames.ps1 }
    Invoke-Checked { cargo build --workspace --release --locked }
    Invoke-Checked { cargo run -p snowflakes-client --release --locked --example snow_model_bench -- --scenario baseline --steps 120 --warmup 30 }
    Invoke-Checked { npm.cmd run test:schema }
    $sfPhpFiles = & rg --files server -g '*.php'
    if ($LASTEXITCODE -ne 0) { throw 'PHP files not found' }
    foreach ($sfFile in $sfPhpFiles) {
        Invoke-Checked { & $Php -l $sfFile }
    }
    Invoke-Checked { & $Php server/tests/run.php }
    if ($SkipHttp) {
        Write-Output 'HTTP: Not run (explicit -SkipHttp)'
    } else {
        Invoke-Checked { & $Php server/tests/http.php }
    }
} finally {
    Pop-Location
}
