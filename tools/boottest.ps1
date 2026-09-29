# boottest.ps1 - build liftoff, boot it under OVMF via QEMU, assert serial output.
# Real-chain acceptance: the assertions below only pass if the firmware actually
# loads BOOTX64.EFI and our code emits the expected lines on COM1.
#
# Usage:
#   pwsh tools/boottest.ps1 -Variant normal  -Expect "M2A: len=36","M2A: sum=0x0834"
#   pwsh tools/boottest.ps1 -Variant missing -Expect "open failed status=0x800000000000000e"
#   pwsh tools/boottest.ps1 -Variant empty   -Expect "M2A: len=0","M2A: sum=0x0000"
param(
    [Parameter(Mandatory = $true)][string[]]$Expect,
    [string[]]$NotExpect = @(),
    [int]$TimeoutSec = 45,
    [ValidateSet("normal", "missing", "empty", "iso", "iso-nosig", "iso-nopath")][string]$Variant = "normal"
)
$ErrorActionPreference = "Stop"

$liftoff = Split-Path -Parent $PSScriptRoot
$project = Split-Path -Parent $liftoff

# QEMU location comes from the project .env (the documented env config file).
# No machine-specific path is hardcoded here; a missing entry is a hard failure.
$qemuDir = $null
foreach ($line in Get-Content (Join-Path $project ".env")) {
    if ($line -match "^QEMU_DIR=(.*)$") { $qemuDir = $Matches[1] }
}
$qemuExe = Join-Path $qemuDir "qemu-system-x86_64.exe"
$fw = Join-Path $qemuDir "share\edk2-x86_64-code.fd"
if (-not $qemuExe -or -not (Test-Path $qemuExe)) { Write-Output "FAIL: QEMU_DIR invalid in .env"; exit 2 }
if (-not (Test-Path $fw)) { Write-Output "FAIL: edk2-x86_64-code.fd not found under QEMU_DIR"; exit 2 }

Write-Output "== cargo build =="
Push-Location $liftoff
cargo build --release --target x86_64-unknown-uefi
$buildRc = $LASTEXITCODE
Pop-Location
if ($buildRc -ne 0) { Write-Output "FAIL: cargo build"; exit 2 }

$efi = Join-Path $liftoff "target\x86_64-unknown-uefi\release\liftoff.efi"
$esp = Join-Path $liftoff "esp"
Remove-Item -Recurse -Force $esp -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path (Join-Path $esp "EFI\BOOT") | Out-Null
Copy-Item $efi (Join-Path $esp "EFI\BOOT\BOOTX64.EFI")

# Test fixture: "LIFTOFF-M2A-TEST\r\n0123456789ABCDEF\r\n" = 36 bytes, byte-sum 0x0834.
# Variant missing: no file at all. Variant empty: zero-byte file.
$fixture = Join-Path $esp "m2a.txt"
switch ($Variant) {
    "normal" { [System.IO.File]::WriteAllBytes($fixture, [System.Text.Encoding]::ASCII.GetBytes("LIFTOFF-M2A-TEST" + [char]13 + [char]10 + "0123456789ABCDEF" + [char]13 + [char]10)) }
    "missing" { }
    "empty"   { [System.IO.File]::WriteAllBytes($fixture, [byte[]]@()) }
}

# ISO9660 variants: build the fixture ISO with the project's own deterministic
# builder (mkiso.py) and attach it as -cdrom. iso-nosig is a non-ISO blob (the
# CD001 probe must fail); iso-nopath is a valid ISO whose root has no KERNEL
# directory (mount ok, open must report not-found).
$iso = $null
$media = "fat"
if ($Variant -like "iso*") {
    $media = "iso"
    $payload = Join-Path $liftoff "target\m2b-payload.bin"
    $iso = Join-Path $liftoff "target\m2b.iso"
    if ($Variant -eq "iso") {
        [System.IO.File]::WriteAllBytes($payload, [System.Text.Encoding]::ASCII.GetBytes("LIFTOFF-M2B-KERNEL" + [char]10 + "0123456789ABCDEF" + [char]10))
        python (Join-Path $PSScriptRoot "mkiso.py") --kernel $payload --out $iso --volident LIFTOFF_M2B
    } elseif ($Variant -eq "iso-nosig") {
        $blob = New-Object byte[] 65536
        (New-Object Random 20260215).NextBytes($blob)
        [System.IO.File]::WriteAllBytes($iso, $blob)
    } elseif ($Variant -eq "iso-nopath") {
        [System.IO.File]::WriteAllBytes($payload, [System.Text.Encoding]::ASCII.GetBytes("LIFTOFF-M2B-KERNEL" + [char]10 + "0123456789ABCDEF" + [char]10))
        python (Join-Path $PSScriptRoot "mkiso.py") --kernel $payload --out $iso --volident EMPTYROOT --flat
    }
    if (-not (Test-Path $iso)) { Write-Output "FAIL: iso fixture not built"; exit 2 }
}

$log = Join-Path $liftoff "target\serial.log"
$qerr = Join-Path $liftoff "target\qemu.err.log"
Remove-Item $log, $qerr -ErrorAction SilentlyContinue

$qargs = @(
    "-drive", "if=pflash,format=raw,readonly=on,file=$fw",
    "-drive", "format=raw,file=fat:rw:$esp",
    "-net", "none",
    "-serial", "file:$log",
    "-display", "none",
    "-no-reboot"
)
if ($media -eq "iso") { $qargs = @("-cdrom", $iso) + $qargs }
$p = Start-Process -FilePath $qemuExe -ArgumentList $qargs -PassThru -RedirectStandardError $qerr
try {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 400
        if ($p.HasExited) { break }
        if (Test-Path $log) {
            $probe = Get-Content $log -Raw -ErrorAction SilentlyContinue
            if ($probe) {
                $allFound = $true
                foreach ($e in $Expect) { if (-not $probe.Contains($e)) { $allFound = $false; break } }
                if ($allFound) { break }
            }
        }
    }
} finally {
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    $p.WaitForExit()
}

$content = if (Test-Path $log) { Get-Content $log -Raw } else { "" }
$failed = $false
foreach ($e in $Expect) {
    if ($content.Contains($e)) { Write-Output ("PASS: contains " + $e) }
    else { Write-Output ("FAIL: missing " + $e); $failed = $true }
}
foreach ($e in $NotExpect) {
    if ($content -and $content.Contains($e)) { Write-Output ("FAIL: unexpectedly present " + $e); $failed = $true }
    else { Write-Output ("PASS: absent " + $e) }
}
if ($failed) {
    Write-Output "== serial log =="
    Write-Output $content
    if (Test-Path $qerr) { Write-Output "== qemu stderr =="; Get-Content $qerr }
    exit 1
}
Write-Output "BOOTTEST PASS"
exit 0