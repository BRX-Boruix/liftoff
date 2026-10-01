//! spinup trampoline（L5）。
//!
//! **为什么需要**：UEFI 环境里直接 jmp 进内核会死 —— 固件留下的分页模式、
//! 段描述符、GDT/IDT、EFER 都是固件的，而内核假设的是 Limine 协议定义的干净状态
//! （真机实测：Exit 成功、激活成功、跳转后即复位循环）。旧实现（brxLimine）的
//! 路径是：把 32 位重置代码**复制到低地址**（Exit 后 64 位 EIP 不可靠），降级到
//! 32 位（关分页）、按协议重设全部机器状态、再重进 64 位并 iretq 进内核。
//!
//! 参考实现：brxLimine common/lib/spinup.asm_uefi_x86_64、
//! common/protos/limine_32.asm_x86、common/lib/misc.c:37-105、sys/gdt.s2.c。
use arch::paging::PageTable;

/// 组装 GDT：与 brxLimine gdt.s2.c 的 8 个描述符逐字段一致（小端 8 字节）。
/// 布局：0 空、1 32 位代码(0x18)、2 32 位数据(0x20)、3 64 位代码(0x28)、
/// 4 64 位数据(0x30)、5 64 位代码(gran=1)、6 64 位数据、7 TSS 低、8 TSS 高。
pub fn build_gdt() -> [u64; 9] {
    // gran 是旧实现里的「granularity」字段：**完整的第 6 字节**（G/D/L 标志 +
    // limit 高 4 位）—— 不要自己拆 limit，直接照抄旧实现的字节值。
    let desc = |limit: u32, base: u32, access: u8, gran: u8| -> u64 {
        let limit_low = (limit & 0xFFFF) as u64;
        let limit_hi = ((limit >> 16) & 0xF) as u64;
        let base_low = (base & 0xFFFF) as u64;
        let base_mid = ((base >> 16) & 0xFF) as u64;
        let base_hi = ((base >> 24) & 0xFF) as u64;
        limit_low
            | (base_low << 16)
            | (base_mid << 32)
            | ((access as u64) << 40)
            | (limit_hi << 48)
            | ((gran as u64) << 52)
            | (base_hi << 56)
    };
    [
        0,
        desc(0xFFFF, 0, 0b1001_1011, 0b0000_0000),
        desc(0xFFFF, 0, 0b1001_0011, 0b0000_0000),
        desc(0xFFFF, 0, 0b1001_1011, 0b1100_1111),
        desc(0xFFFF, 0, 0b1001_0011, 0b1100_1111),
        desc(0, 0, 0b1001_1011, 0b0010_0000),
        desc(0, 0, 0b1001_0011, 0),
        desc(0, 0, 0x89, 0),
        0,
    ]
}

#[cfg(test)]
mod tests {
    use super::build_gdt;

    #[test]
    fn gdt_entries_match_the_reference_layout() {
        let gdt = build_gdt();
        assert_eq!(gdt[0], 0, "描述符 0 必须为空");
        // 32 位代码 (0x18)：access=10011011b=0x9B, gran 字节=0
        assert_eq!(gdt[1] >> 40 & 0xFF, 0x9B);
        // 32 位数据 (0x20)：access=10010011b=0x93
        assert_eq!(gdt[2] >> 40 & 0xFF, 0x93);
        // 64 位代码 (0x28)：access=0x9B，gran 字节 = 0b1100_1111（G=1、D=0、L=1、limit_hi=1111）
        assert_eq!(gdt[3] >> 40 & 0xFF, 0x9B);
        assert_eq!(gdt[3] >> 52 & 0xFF, 0b1100_1111);
        // 64 位数据 (0x30)：access=0x93
        assert_eq!(gdt[4] >> 40 & 0xFF, 0x93);
        // TSS（0x38）：access=0x89
        assert_eq!(gdt[7] >> 40 & 0xFF, 0x89);
    }
}