$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
    cargo build --release --locked --offline
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
    python scripts/demo.py --binary target/release/kvrecover.exe
    if ($LASTEXITCODE -ne 0) { throw 'Demonstration failed' }
} finally { Pop-Location }
