#!/usr/bin/env python3
"""M8 模块（ModuleRequest）验收：fixture 生成 + 期望值计算（单一来源）。

生成确定性的模块文件（供 mkiso 打包），并输出 modtest 消费者应当打印的
契约行（前缀 MOD:）。模块表必须与 liftoff/src/config.rs 的 MODULES 一致；
若不一致，modtest 打印的 cmd 字段会导致断言失败（自检）。
"""
import argparse, os, sys

# (ISO 内路径, 命令行, 尺寸, 字节生成式) —— 必须与 config.rs::MODULES 对应。
MODULES = [
    ("/MODULES/ALPHA.BIN", "role=alpha", 4096, lambda i: (i * 7 + 3) & 0xFF),
    ("/MODULES/BETA.BIN", "role=beta", 8192, lambda i: (i * 13 + 5) & 0xFF),
]

def emit(dirpath: str):
    paths = []
    for path, cmd, size, gen in MODULES:
        name = "mod-" + os.path.basename(path).lower()
        full = os.path.join(dirpath, name)
        data = bytes(gen(i) for i in range(size))
        with open(full, "wb") as f:
            f.write(data)
        paths.append((path, cmd, data))
    return paths

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fixtures", required=True, help="fixture output directory")
    a = ap.parse_args()
    os.makedirs(a.fixtures, exist_ok=True)
    files = emit(a.fixtures)

    out = []
    out.append("[modtest] alive")
    out.append("[modtest] baserev=0")
    out.append(f"[modtest] count={len(files)}")
    for i, (path, cmd, data) in enumerate(files):
        out.append(
            f"[modtest] m{i} path={path} len={len(data)} media=1 "
            f"sum16=0x{sum(data) & 0xFFFF:04x} cmd={cmd}"
        )
    out.append("[modtest] done")
    # liftoff 侧观察行（EBS 前，串口可靠）
    for i, (path, cmd, data) in enumerate(files):
        out.append(
            f"[m8] module {i} path={path} len={len(data)} "
            f"sum16=0x{sum(data) & 0xFFFF:04x}"
        )

    for line in out:
        print("MOD: " + line)
    for path, cmd, data in files:
        print(f"fixture={path} len={len(data)} sum16=0x{sum(data) & 0xFFFF:04x}")

if __name__ == "__main__":
    sys.exit(main())
