# boottest.ps1 - build liftoff, boot it under OVMF via QEMU, assert serial output.
# Real-chain acceptance: the assertions below only pass if the firmware actually
# loads BOOTX64.EFI and our code emits the expected lines on COM1.
#
# Usage:
#   pwsh tools/boottest.ps1 -Variant normal  -Expect "M2A: len=36","M2A: sum=0x0834"
#   pwsh tools/boottest.ps1 -Variant missing -Expect "open failed status=0x800000000000000e"
#   pwsh tools/boottest.ps1 -Variant empty   -Expect "M2A: len=0","M2A: sum=0x0000"
param(
    [Parameter(Mandatory = $false)][string[]]$Expect = @(),
    [string[]]$NotExpect = @(),
    [int]$TimeoutSec = 45,
    [ValidateSet("normal", "missing", "empty", "iso", "iso-nosig", "iso-nopath", "ext", "ext-nosig", "ext-nopath", "elf-iso", "elf-bad", "boot")][string]$Variant = "normal"
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
# EAP=Stop makes native stderr fatal when the caller redirects streams; run via
cmd /c "cargo build --release --target x86_64-unknown-uefi 2>&1"
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
if ($Variant -like "iso*" -or $Variant -like "elf-*" -or $Variant -eq "boot") {
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
    } elseif ($Variant -eq "elf-bad") {
        # ELF validation negative: deterministic pseudo-random blob (no magic).
        # The ISO chain reads it fine; elf::load must reject with 0x30 BAD_MAGIC.
        $blob = New-Object byte[] 4096
        (New-Object Random 99).NextBytes($blob)
        [System.IO.File]::WriteAllBytes($payload, $blob)
        python (Join-Path $PSScriptRoot "mkiso.py") --kernel $payload --out $iso --volident BADELF
    } elseif ($Variant -eq "boot") {
        # Full handover: real kernel ELF, liftoff builds paging + BORUIX v1
        # responses, exits boot services and jumps. Kernel prints its banner.
        $kernelElf = Join-Path $liftoff "target\kernel.elf"
        python (Join-Path $PSScriptRoot "mkiso.py") --kernel $kernelElf --out $iso --volident LIFTOFF_BOOT
    } elseif ($Variant -eq "elf-iso") {
        # Real kernel ELF packed as KERNEL/KERNIMG.BIN; expectations derive from
        # elf_oracle.py at run time (the ELF changes with every kernel rebuild).
        $kernelElf = Join-Path $liftoff "target\kernel.elf"
        python (Join-Path $PSScriptRoot "mkiso.py") --kernel $kernelElf --out $iso --volident LIFTOFF_M3
    }
    if (-not (Test-Path $iso)) { Write-Output "FAIL: iso fixture not built"; exit 2 }
}

# elf-iso: derive the assertion list from the oracle (single source of expectations).
$autoExpect = $null
if ($Variant -eq "elf-iso") {
    $oracleOut = python (Join-Path $PSScriptRoot "elf_oracle.py") (Join-Path $liftoff "target\kernel.elf") 2>&1 | ForEach-Object { $_.ToString() }
    $autoExpect = @($oracleOut | Where-Object { $_ -match '^M3: ' })
    if ($autoExpect.Count -lt 6) { Write-Output "FAIL: oracle produced too few lines"; exit 2 }
}
# boot: full-chain anchors. Each line appears only if a distinct handover
# stage works: banner (jump), HHDM offset (protocol), RSDP rev=2 (M5 config
# table), pmm sizing on a sane map (memmap pointer-array semantics).
if ($Variant -eq "boot") {
    $autoExpect = @("Hello, BORUIX!", "Kernel M0 is running.", "[mm] HHDM offset: 0xffff800000000000", "[acpi] RSDP rev=2", "LazyBuddy init done")
}

# EXT2 variants: whole-disk fixture from mkext2.py attached as a plain raw drive.
# ext: /BOOT/KERNIMG.BIN present; ext-nosig: non-EXT2 blob (magic must fail);
# ext-nopath: valid EXT2 whose root has no BOOT directory.
$extimg = $null
if ($Variant -like "ext*") {
    $media = "ext"
    $payload = Join-Path $liftoff "target\m2c-payload.bin"
    $extimg = Join-Path $liftoff "target\m2c.img"
    if ($Variant -eq "ext") {
        [System.IO.File]::WriteAllBytes($payload, [System.Text.Encoding]::ASCII.GetBytes("LIFTOFF-M2C-KERNEL" + [char]13 + [char]10 + "0123456789ABCDEF" + [char]13 + [char]10))
        python (Join-Path $PSScriptRoot "mkext2.py") --kernel $payload --out $extimg
    } elseif ($Variant -eq "ext-nosig") {
        $blob = New-Object byte[] 2097152
        (New-Object Random 20260216).NextBytes($blob)
        [System.IO.File]::WriteAllBytes($extimg, $blob)
    } elseif ($Variant -eq "ext-nopath") {
        [System.IO.File]::WriteAllBytes($payload, [System.Text.Encoding]::ASCII.GetBytes("LIFTOFF-M2C-KERNEL" + [char]13 + [char]10 + "0123456789ABCDEF" + [char]13 + [char]10))
        python (Join-Path $PSScriptRoot "mkext2.py") --kernel $payload --out $extimg --flat
    }
    if (-not (Test-Path $extimg)) { Write-Output "FAIL: ext fixture not built"; exit 2 }
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
$effExpect = $Expect
if ($autoExpect) { $effExpect = @($Expect) + $autoExpect }
if ($media -eq "iso") { $qargs = @("-cdrom", $iso) + $qargs }
if ($media -eq "ext") { $qargs = $qargs + @("-drive", "format=raw,file=$extimg") }
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
                foreach ($e in $effExpect) { if (-not $probe.Contains($e)) { $allFound = $false; break } }
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
foreach ($e in $effExpect) {
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