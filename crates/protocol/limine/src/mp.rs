//! Limine SMP（mp）协议。
//!
//! 只声明 **x86_64 变体**：limine.h 里该结构按架构条件编译，其它架构另有其形
//! （例如 loongarch64 的 flags 是 u64、字段名是 bsp_phys_id）。
//! 已对照 brxLimine/limine-protocol/include/limine.h 核实（2026-09-30）。

use crate::base::COMMON_MAGIC;

/// `LIMINE_MP_REQUEST_ID`。
pub const MP_REQUEST_ID: [u64; 4] = [
    COMMON_MAGIC[0],
    COMMON_MAGIC[1],
    0x95a67b819a1b857e,
    0xa0b61b723b6a73e0,
];

/// `LIMINE_MP_REQUEST_X86_64_X2APIC`。
pub const MP_REQUEST_X86_64_X2APIC: u64 = 1;

/// `LIMINE_MP_RESPONSE_X86_64_X2APIC`。
pub const MP_RESPONSE_X86_64_X2APIC: u32 = 1;

/// `limine_goto_address`：AP 的入口函数指针。
pub type GotoAddress = unsafe extern "C" fn(info: *mut MpInfo);

/// `struct limine_mp_info`（x86_64）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MpInfo {
    /// 处理器标识。
    pub processor_id: u32,
    /// 本地 APIC 标识。
    pub lapic_id: u32,
    /// 保留。
    pub reserved: u64,
    /// AP 入口（内核填写；空表示未启动）。
    pub goto_address: Option<GotoAddress>,
    /// 传给 AP 的额外参数。
    pub extra_argument: u64,
}

/// `struct limine_mp_response`（x86_64）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MpResponse {
    /// 响应修订。
    pub revision: u64,
    /// 响应标志（见 `MP_RESPONSE_*`）。
    pub flags: u32,
    /// 启动处理器的本地 APIC 标识。
    pub bsp_lapic_id: u32,
    /// CPU 数量。
    pub cpu_count: u64,
    /// 指向 CPU 信息指针数组的指针。
    pub cpus: *mut *mut MpInfo,
}

/// `struct limine_mp_request`（比其它请求多一个 flags 字段）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MpRequest {
    /// 请求标识。
    pub id: [u64; 4],
    /// 请求修订。
    pub revision: u64,
    /// 响应指针（由引导器填充）。
    pub response: *mut MpResponse,
    /// 请求标志（见 `MP_REQUEST_*`）。
    pub flags: u64,
}

#[cfg(test)]
mod tests {
    use super::{
        MP_REQUEST_ID, MP_REQUEST_X86_64_X2APIC, MP_RESPONSE_X86_64_X2APIC, MpInfo, MpRequest,
        MpResponse,
    };
    use core::mem::{offset_of, size_of};

    #[test]
    fn info_layout_matches_the_x86_64_variant() {
        // { u32 processor_id; u32 lapic_id; u64 reserved; goto_address; u64 extra_argument; }
        assert_eq!(size_of::<MpInfo>(), 32);
        assert_eq!(offset_of!(MpInfo, processor_id), 0);
        assert_eq!(offset_of!(MpInfo, lapic_id), 4);
        assert_eq!(offset_of!(MpInfo, reserved), 8);
        assert_eq!(offset_of!(MpInfo, goto_address), 16);
        assert_eq!(offset_of!(MpInfo, extra_argument), 24);
    }

    #[test]
    fn response_layout_matches_the_x86_64_variant() {
        // flags and bsp_lapic_id are 32-bit here (unlike the loongarch64 variant).
        assert_eq!(size_of::<MpResponse>(), 32);
        assert_eq!(offset_of!(MpResponse, revision), 0);
        assert_eq!(offset_of!(MpResponse, flags), 8);
        assert_eq!(offset_of!(MpResponse, bsp_lapic_id), 12);
        assert_eq!(offset_of!(MpResponse, cpu_count), 16);
        assert_eq!(offset_of!(MpResponse, cpus), 24);
    }

    #[test]
    fn request_has_an_extra_flags_field() {
        assert_eq!(size_of::<MpRequest>(), 56);
        assert_eq!(offset_of!(MpRequest, response), 40);
        assert_eq!(offset_of!(MpRequest, flags), 48);
    }

    #[test]
    fn ids_and_flags_match_the_header() {
        assert_eq!(MP_REQUEST_ID[2], 0x95a67b819a1b857e);
        assert_eq!(MP_REQUEST_ID[3], 0xa0b61b723b6a73e0);
        assert_eq!(MP_REQUEST_X86_64_X2APIC, 1);
        assert_eq!(MP_RESPONSE_X86_64_X2APIC, 1);
    }
}
