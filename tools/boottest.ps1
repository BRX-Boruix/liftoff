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
    [ValidateSet("normal", "missing", "empty", "iso", "iso-nosig", "iso-nopath", "ext", "ext-nosig", "ext-nopath", "elf-iso", "elf-bad", "boot", "mod", "ext-boot", "ext-mod", "smp4", "x2apic")][string]$Variant = "normal"
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
if ($Variant -like "iso*" -or $Variant -like "elf-*" -or $Variant -eq "boot" -or $Variant -eq "mod" -or $Variant -eq "smp4" -or $Variant -eq "x2apic") {
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
    } elseif ($Variant -eq "boot" -or $Variant -eq "smp4" -or $Variant -eq "x2apic") {
        # Full handover: real kernel ELF, liftoff builds paging + BORUIX v1
        # responses, exits boot services and jumps. Kernel prints its banner.
        # smp4 uses the same image with -smp 4 (M11 AP scale-out).
        $kernelElf = Join-Path $liftoff "target\kernel.elf"
        python (Join-Path $PSScriptRoot "mkiso.py") --kernel $kernelElf --out $iso --volident LIFTOFF_BOOT
    } elseif ($Variant -eq "mod") {
        # M8 modules: the payload kernel is the standalone consumer in
        # tools/modtest (declares BaseRevision + ModuleRequest). Module files
        # and expectations come from modtest_oracle.py (single source).
        & (Join-Path $PSScriptRoot "modtest\build.ps1") | Out-Null
        $payload = Join-Path $liftoff "tools\modtest\modtest.elf"
        if (-not (Test-Path $payload)) { Write-Output "FAIL: modtest kernel build failed"; exit 2 }
        python (Join-Path $PSScriptRoot "modtest_oracle.py") --fixtures (Join-Path $liftoff "target") | Out-Null
        $alpha = Join-Path $liftoff "target\mod-alpha.bin"
        $beta = Join-Path $liftoff "target\mod-beta.bin"
        python (Join-Path $PSScriptRoot "mkiso.py") --kernel $payload --out $iso --volident LIFTOFF_M8 `
            --extra "MODULES/ALPHA.BIN=$alpha" --extra "MODULES/BETA.BIN=$beta" | Out-Null
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
# Small variants carry their own contract anchors, so "-Variant X" alone is a real
# test (the harness hard-fails on an empty expectation list).
switch ($Variant) {
    "normal"     { $autoExpect = @("M2A: len=36", "M2A: sum=0x0834") }
    "missing"    { $autoExpect = @("M2A: open failed status=0x800000000000000e") }
    "empty"      { $autoExpect = @("M2A: len=0", "M2A: sum=0x0000") }
    "iso"        { $autoExpect = @("M2B: mount ok", "M2B: len=36", "M2B: sum=0x089c") }
    "iso-nosig"  { $autoExpect = @("M2B: mount failed status=0x11") }
    "iso-nopath" { $autoExpect = @("M2B: open failed status=0x800000000000000e") }
    "elf-bad"    { $autoExpect = @("M3: reject status=0x30") }
    "ext"        { $autoExpect = @("M2C: mount ok", "M2C: len=38", "M2C: sum=0x08b7") }
    "ext-nosig"  { $autoExpect = @("M2C: mount failed status=0x21") }
    "ext-nopath" { $autoExpect = @("M2C: open failed status=0x800000000000000e") }
}
if ($Variant -eq "elf-iso") {
    $oracleOut = python (Join-Path $PSScriptRoot "elf_oracle.py") (Join-Path $liftoff "target\kernel.elf") 2>&1 | ForEach-Object { $_.ToString() }
    $autoExpect = @($oracleOut | Where-Object { $_ -match '^M3: ' })
    if ($autoExpect.Count -lt 6) { Write-Output "FAIL: oracle produced too few lines"; exit 2 }
}
# boot: full-chain anchors. Each line appears only if a distinct handover
# stage works: banner (jump), HHDM offset (protocol), RSDP rev=2 (M5 config
# table), pmm sizing on a sane map (memmap pointer-array semantics).
if ($Variant -eq "boot") {
    $autoExpect = @("Hello, BORUIX!", "Kernel M0 is running.", "[mm] HHDM offset: 0xffff800000000000", "[acpi] RSDP rev=2", "LazyBuddy init done", "[smp] BSP lapic_id=0", "[smp] fired AP lapic_id=1", "[smp] AP online, lapic_id=1")
}
if ($Variant -eq "smp4") {
    # M11: four CPUs. Each line proves a distinct AP was brought up by liftoff
    # (fired) and then taken over by the kernel (online).
    $autoExpect = @(
        "Hello, BORUIX!",
        "Kernel M0 is running.",
        "[mm] HHDM offset: 0xffff800000000000",
        "[acpi] RSDP rev=2",
        "LazyBuddy init done",
        "[smp] BSP lapic_id=0",
        "[smp] fired AP lapic_id=1",
        "[smp] fired AP lapic_id=2",
        "[smp] fired AP lapic_id=3",
        "[smp] AP online, lapic_id=1",
        "[smp] AP online, lapic_id=2",
        "[smp] AP online, lapic_id=3",
        "[kmain] SMP done, 4 cpus online (target 4)",
        "[m6] x2apic supported=false enabled=false"   # 默认 qemu64 CPU：xAPIC 回退路径
    )
}
if ($Variant -eq "x2apic") {
    # M12: x2APIC path. -cpu max exposes x2APIC, so liftoff switches the LAPIC
    # to MSR access, reports it in the SMP response flags, and the kernel picks
    # the same mode ([lapic] x2APIC mode) - four CPUs must still come online.
    $autoExpect = @(
        "Hello, BORUIX!",
        "Kernel M0 is running.",
        "[mm] HHDM offset: 0xffff800000000000",
        "[acpi] RSDP rev=2",
        "LazyBuddy init done",
        "[m6] x2apic supported=true enabled=true",
        "[lapic] x2APIC mode: MSR access",
        "[smp] BSP lapic_id=0",
        "[smp] fired AP lapic_id=1",
        "[smp] fired AP lapic_id=2",
        "[smp] fired AP lapic_id=3",
        "[smp] AP online, lapic_id=1",
        "[smp] AP online, lapic_id=2",
        "[smp] AP online, lapic_id=3",
        "[kmain] SMP done, 4 cpus online (target 4)"
    )
}
if ($Variant -eq "mod" -or $Variant -eq "ext-mod") {
    # M8/M10: expectations are derived by the oracle from the same fixture bytes
    # that mkiso/mkext2 pack (module paths/lengths/sums/cmdlines).
    $oracleOut = python (Join-Path $PSScriptRoot "modtest_oracle.py") --fixtures (Join-Path $liftoff "target") 2>&1 | ForEach-Object { $_.ToString() }
    $autoExpect = @($oracleOut | Where-Object { $_ -match '^MOD: ' } | ForEach-Object { $_.Substring(5) })
    if ($autoExpect.Count -lt 8) { Write-Output "FAIL: modtest oracle produced too few lines"; exit 2 }
}
# ext-boot (M9/M13): install-mode end to end from an MBR system disk that this
# script builds itself (tools/mksysdisk.py). Every line proves a distinct stage:
# MBR/partition selection, EXT2 mount at the partition offset, the kernel read
# through double indirect blocks, ELF handover, and the kernel choosing install
# mode from the BootSource we filled.
if ($Variant -eq "ext-boot") {
    # Device-independent anchors: disk id + partition index + start LBA come
    # from the MBR we wrote; the kernel echoes them back in install mode.
    $autoExpect = @(
        "[m2c] mbr disk_id=0x424f5255 partition=1 start_lba=2048",
        "M2C: mount ok",
        "[m9] kernel path=/boot/kernel",
        "M3: segs=3 entry=",
        "Hello, BORUIX!",
        "Kernel M0 is running.",
        "[mm] HHDM offset: 0xffff800000000000",
        "[acpi] RSDP rev=2",
        "[boot] install mode detected: boot disk mbr_disk_id=0x424f5255 partition_index=1",
        "partition lba=2048 (EXT2)",
        "LazyBuddy init done"
    )
}
if ($Variant -eq "ext-boot") {
    # M13: build the install disk here instead of requiring a workspace artifact.
    # Same layout the project toolchain produces (MBR disk id 0x424F5255,
    # partition 1 at LBA 2048, EXT2 with /boot/kernel), so the anchors below
    # stay byte-for-byte identical to the ones verified against systemdisk.img.
    $media = "ext"
    $kernelElf = Join-Path $liftoff "target\kernel.elf"
    if (-not (Test-Path $kernelElf)) { Write-Output "FAIL: target/kernel.elf missing (build the BORUIX kernel first)"; exit 2 }
    $extimg = Join-Path $liftoff "target\systemdisk.img"
    python (Join-Path $PSScriptRoot "mksysdisk.py") --kernel $kernelElf --out $extimg | Out-Null
    if (-not (Test-Path $extimg)) { Write-Output "FAIL: mksysdisk produced no image"; exit 2 }
} elseif ($Variant -like "ext*") {
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
    } elseif ($Variant -eq "ext-mod") {
        # M10: modules over EXT2 (install mode). Payload kernel = the standalone
        # consumer in tools/modtest; module fixtures and expectations come from
        # modtest_oracle.py (same bytes as the ISO "mod" variant).
        & (Join-Path $PSScriptRoot "modtest\build.ps1") | Out-Null
        python (Join-Path $PSScriptRoot "modtest_oracle.py") --fixtures (Join-Path $liftoff "target") | Out-Null
        $payload = Join-Path $liftoff "tools\modtest\modtest.elf"
        if (-not (Test-Path $payload)) { Write-Output "FAIL: modtest kernel build failed"; exit 2 }
        $alpha = Join-Path $liftoff "target\mod-alpha.bin"
        $beta = Join-Path $liftoff "target\mod-beta.bin"
        python (Join-Path $PSScriptRoot "mkext2.py") --kernel $payload --out $extimg `
            --extra "MODULES/ALPHA.BIN=$alpha" --extra "MODULES/BETA.BIN=$beta" | Out-Null
    } elseif ($Variant -eq "ext-nopath") {
        [System.IO.File]::WriteAllBytes($payload, [System.Text.Encoding]::ASCII.GetBytes("LIFTOFF-M2C-KERNEL" + [char]13 + [char]10 + "0123456789ABCDEF" + [char]13 + [char]10))
        python (Join-Path $PSScriptRoot "mkext2.py") --kernel $payload --out $extimg --flat
    }
    if (-not (Test-Path $extimg)) { Write-Output "FAIL: ext fixture not built"; exit 2 }
}

$snapshot = $null
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
if ($effExpect.Count -eq 0) {
    # 无断言的变体绝不能报 PASS：那会让"测试没跑"伪装成"测试通过"。
    Write-Output "FAIL: variant $Variant produced no expectations"; exit 2
}
if ($Variant -eq "boot") { $qargs = @("-smp", "2") + $qargs }
if ($Variant -eq "smp4") { $qargs = @("-smp", "4") + $qargs }
if ($Variant -eq "x2apic") {
    # Minimal CPU model plus exactly the x2APIC feature: isolates the MSR LAPIC
    # path from the extra features -cpu max would expose (which trip unrelated
    # kernel-side issues).
    $qargs = @("-smp", "4", "-cpu", "qemu64,+x2apic") + $qargs
}
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
                if ($allFound) {
                    # 命中即快照：QEMU 被强杀时 -serial file: 的缓冲可能未落盘，
                    # 事后重读会拿到截断日志（实测只剩 87 字节）。
                    $snapshot = $probe
                    break
                }
            }
        }
    }
} finally {
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
    $p.WaitForExit()
}

$content = if ($snapshot) { $snapshot } elseif (Test-Path $log) { Get-Content $log -Raw } else { "" }
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