# Builds release and refreshes dist\YKExplorer.exe.
#
# The distributable must run at normal (Medium) integrity: a copy carrying the
# "Low Mandatory Level" label cannot write HKCU, so "Replace Explorer" would
# fail with access denied (5). The script tries to relabel the output and warns
# when the surrounding folder is still sandboxed.

$ErrorActionPreference = 'Stop'

cargo build --release
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

New-Item -ItemType Directory -Force -Path 'dist' | Out-Null
Copy-Item 'target\release\YKExplorer.exe' 'dist\YKExplorer.exe' -Force

$exeAcl = (icacls 'dist\YKExplorer.exe') -join "`n"
if ($exeAcl -match 'Low Mandatory') {
    # Try to raise the label; this can fail when the build ran in a sandbox.
    icacls 'dist\YKExplorer.exe' /setintegritylevel M 2>&1 | Out-Null
    $exeAcl = (icacls 'dist\YKExplorer.exe') -join "`n"
}
if ($exeAcl -match 'Low Mandatory') {
    Write-Warning 'dist\YKExplorer.exe still has the Low Mandatory Level label.'
    Write-Warning 'Windows will deny HKCU writes (error 5). Copy the exe to a normal folder (Desktop or %LOCALAPPDATA%) and run it there.'
} else {
    Write-Host 'Built dist\YKExplorer.exe (normal integrity).'
}
