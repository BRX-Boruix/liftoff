# 构建 M8 模块协议验收用的测试内核（独立消费者）。
# 产物：tools/modtest/modtest.elf（静态 ET_EXEC，高半 vbase，无动态重定位）。
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$out = Join-Path $here 'modtest.elf'
$ld = Join-Path $here 'linker.ld'
$src = Join-Path $here 'main.rs'
& rustc --edition 2021 -O --target x86_64-unknown-none -C panic=abort `
    -C relocation-model=static -C link-arg=-nostdlib -C link-arg=--build-id=none `
    -C "link-arg=-T$ld" --crate-type bin -o $out $src
if ($LASTEXITCODE -ne 0) { Write-Output 'FAIL: modtest build'; exit 2 }
$len = (Get-Item $out).Length
$sum = 0
foreach ($b in [System.IO.File]::ReadAllBytes($out)) { $sum = ($sum + $b) % 65536 }
Write-Output ('modtest_bytes=' + $len + ' sum16=0x' + $sum.ToString('x4'))
