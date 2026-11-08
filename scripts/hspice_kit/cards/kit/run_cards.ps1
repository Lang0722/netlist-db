# Runs HSPICE on each deck of decks\, from inside decks\ (so that the .lib
# file next to the decks is found), writes results\, and packs them into
# hspice_card_results.zip.
#   powershell -NoProfile -ExecutionPolicy Bypass -File run_cards.ps1
# Set $env:HSPICE to the path of hspice.exe first if it is not on the PATH
# (for example C:\synopsys\Hspice_P-2019.06-SP1-1\WIN64\hspice.exe).
$ErrorActionPreference = "Continue"
$kit = Split-Path -Parent $MyInvocation.MyCommand.Path
$hspice = if ($env:HSPICE) { $env:HSPICE } else { "hspice" }
$results = Join-Path $kit "results"
New-Item -ItemType Directory -Force -Path $results | Out-Null
Push-Location (Join-Path $kit "decks")
try {
    & $hspice -v *> (Join-Path $results "hspice_version.txt")
    foreach ($deck in Get-ChildItem -Filter *.sp | Sort-Object Name) {
        $name = $deck.BaseName
        Write-Host "== $name"
        & $hspice -i $deck.Name -o (Join-Path $results $name) *> (Join-Path $results "$name.stdout")
        if ($LASTEXITCODE -ne 0) {
            Write-Host "   (hspice exited with status $LASTEXITCODE: see results\$name.stdout and results\$name.lis)"
        }
    }
} finally {
    Pop-Location
}
Compress-Archive -Force -Path $results -DestinationPath (Join-Path $kit "hspice_card_results.zip")
Write-Host "done: hspice_card_results.zip"
