# 外部依赖台账（liftoff）

按 S35：每引入一个外部依赖，记录**引入原因 / 替代方案评估 / 版本锁定策略 / 退出计划**。
许可归属不在此重复 —— 见 `NOTICE.md`（那里是许可的事实来源，避免两处各写一份而分叉）。

---

## 当前状态（第 256 轮核查）

**liftoff 没有任何外部 crate 依赖。** 全部 `[dependencies]` 条目都是本仓库内的
`path` 依赖（`arch` / `limine` / `firmware` / `current` / `mm` / `fs` / `driver` /
`loader` / `efi` 等）。核查方法：逐 crate 读 `Cargo.toml`，凡依赖条目不含 `path =`
即为外部依赖 —— 结果为空。

这意味着没有 `Cargo.lock` 层面的供应链风险，也没有第三方版本漂移问题。**这是
有意维持的状态**：引导器运行在固件交出控制权之后、内核接管之前，任何第三方代码
都在这个窗口里以 ring 0 执行，能少一份就少一份。

---

## 依赖 1：Rust 工具链（nightly + `x86_64-unknown-uefi`）

- **引入原因**：UEFI 目标的裸机编译需要 nightly。`rust-toolchain.toml` 声明
  `components = ["rust-src", "llvm-tools"]`、`targets = ["x86_64-unknown-uefi"]`。
- **替代方案评估**：稳定版 Rust 目前无法满足该目标的裸机构建需求；换 C/汇编会放弃
  整个仓库的类型安全与抽象边界（ADR-049/050）。**不换。**
- **版本锁定策略**：**`channel = "nightly"`，不锁日期 —— 这是所有者第 106 轮的裁定** ✓。
  **依据**：锁定日期会与 `kernel` 仓库**耦合**（两仓库同源工具链），而工具链日期属于 kernel 侧的
  构建配置，**在 liftoff 的边界之外**（本仓库不得读/改 kernel 源码）✓。在不知道 kernel 用哪个
  nightly 的前提下，liftoff 单方面锁一个日期会让两仓库的工具链**互相打架**，比不锁更坏 ✗。
  **已知代价（如实记录，不粉饰）**：不锁日期意味着**今天能编过、明天可能编不过**，构建不可复现，
  且失败原因会伪装成「代码问题」✗。这个代价被**有意接受** ✓。
  **重新评估的触发条件**：若 kernel 仓库确定了 nightly 日期，liftoff 应当**跟随同一个日期**
  （而不是自己挑一个）—— 那时把 `channel` 改成 `nightly-YYYY-MM-DD`，升级 = 显式动作 + 重跑全部三层验证 ✓。
- **退出计划**：若将来该目标在稳定版上可用，把 `channel` 改为具体稳定版本号即可；
  本仓库不依赖任何 nightly-only 的语言特性（除目标本身）。

---

## 依赖 2：`crates/flanterm_rust`（vendored 终端库）

- **引入原因**：framebuffer 终端（像素写入与字形渲染）。与 `kernel` 仓库的
  `vendor/flanterm_rust/` **同源**。
- **许可**：新增/改写的 Rust 代码 MIT；派生自上游 flanterm 的部分 BSD-2-Clause。
  许可文件在 `crates/flanterm_rust/LICENSE-MIT`、`LICENSE-BSD-2`；归属说明见 `NOTICE.md`。
- **⚠️ 当前实际状态（必须如实说明）**：**它被排除在 workspace 之外**
  （`Cargo.toml` 里 `exclude = ["crates/flanterm_rust"]`），**且没有任何 crate 依赖它**。
  仓库里仅有的引用是两处文档注释（`crates/efi/src/graphics.rs` 与
  `crates/firmware/src/graphics.rs`）写着「像素写入由它和上层负责」
  —— 那是**意图**，不是当前事实。
- **替代方案评估**：自己写终端渲染器 = 重造轮子且要自己处理 Unicode 宽度、字形、
  转义序列；引入 crates.io 上的 `flanterm` = 引入真正的第三方供应链依赖。
  vendored + 双许可 + 与 kernel 同源，在当前取舍下是合理的。
- **退出计划**：它是**纯 vendored 源码**（无 Cargo 依赖），移除 = 删目录 + 清理
  `Cargo.toml` 的 `exclude` 与 `README`/`README.en`/`NOTICE.md` 的相应条目。**成本极低。**
- **所有者裁定（第 106 轮）：保留** ✓。
  **依据**：与参考实现一致 —— brxLimine 顶层同样 **vendor 了 `flanterm/`** ✓（本会话已核实其存在 ✓）。
  vendored 源码**离线可构建、无网络依赖**，是正当做法 ✓；而"自己写终端渲染器"等于**重造轮子**
  （要自己处理 Unicode 宽度、字形、转义序列）✗，引入 crates.io 上的 `flanterm` 则是真正的
  第三方供应链依赖 ✗。
  **现状保持**：它仍在 workspace 之外、仍没有 crate 依赖它 ✓ —— 那是"**接线时再加入成员**"的待办，
  不是"要不要留"的问题 ✓。**接入终端渲染时把它加入 workspace 成员** ✓。

---

## 依赖 3：QEMU + OVMF（仅测试期）

- **引入原因**：PRE-2 端到端验收需要真实固件与虚拟硬件（ADR-052 第 1 层）。
- **位置**：**不属于 liftoff 的构建依赖**，由 `tools` 侧从 `envfiles/` 取（`QEMU_DIR`）。
  liftoff 自身在无 QEMU 时照常编译。
- **版本锁定策略**：由 `envfiles/tools/qemu-stable/` 固定；`tools_build/liftoff.py`
  通过 `.env` 的 `QEMU_DIR` 读取，缺项即硬失败（不静默回退到 PATH 上的任意版本）。
- **退出计划**：不适用（测试基础设施，不进入交付物）。
