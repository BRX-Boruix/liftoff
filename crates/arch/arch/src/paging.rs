//! 分页语义接口：把"建立映射"表达为与架构无关的区间操作。
//!
//! 抽象边界（ADR-007）：调用方只声明区间与权限，页表层数与页大小由实现决定，
//! 因此本模块不出现 PML4/PD 之类的架构名词。

use crate::addr::{Alignment, PhysAddr, PhysFrame, VirtAddr};

/// 页表项权限（语义位，单点定义；实现负责映射到本架构的位布局）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageFlags(u64);

const FLAG_PRESENT: u64 = 1 << 0;
const FLAG_WRITABLE: u64 = 1 << 1;
const FLAG_EXECUTABLE: u64 = 1 << 2;

impl PageFlags {
    /// 无权限位。
    #[inline]
    pub const fn none() -> Self {
        Self(0)
    }

    /// 页存在。
    #[inline]
    pub const fn present() -> Self {
        Self(FLAG_PRESENT)
    }

    /// 可写。
    #[inline]
    pub const fn writable() -> Self {
        Self(FLAG_WRITABLE)
    }

    /// 允许执行（默认不可执行的架构上表示"允许"）。
    #[inline]
    pub const fn executable() -> Self {
        Self(FLAG_EXECUTABLE)
    }

    /// 合并两组权限。
    #[inline]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// 是否置了"存在"位。
    #[inline]
    pub const fn is_present(self) -> bool {
        self.0 & FLAG_PRESENT != 0
    }

    /// 是否置了"可写"位。
    #[inline]
    pub const fn is_writable(self) -> bool {
        self.0 & FLAG_WRITABLE != 0
    }

    /// 是否置了"允许执行"位。
    #[inline]
    pub const fn is_executable(self) -> bool {
        self.0 & FLAG_EXECUTABLE != 0
    }
}

/// 建立映射时的失败原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapError {
    /// 区间长度为零。
    Empty,
    /// 虚拟地址未按对齐量对齐。
    MisalignedVirt,
    /// 物理地址未按对齐量对齐。
    MisalignedPhys,
    /// 长度不是对齐量的整数倍。
    MisalignedLength,
    /// 区间端点计算溢出。
    Overflow,
    /// 页帧分配耗尽（实现方的帧来源无可用帧）。
    OutOfMemory,
    /// 页表帧不在直接映射内，无法访问。
    TableNotAccessible,
    /// 参数合法，但实现不支持该页粒度。
    UnsupportedGranularity,
}

impl core::fmt::Display for MapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            Self::Empty => "区间长度为零",
            Self::MisalignedVirt => "虚拟地址未对齐",
            Self::MisalignedPhys => "物理地址未对齐",
            Self::MisalignedLength => "长度不是对齐量的整数倍",
            Self::Overflow => "区间端点计算溢出",
            Self::OutOfMemory => "页帧分配耗尽",
            Self::TableNotAccessible => "页表帧不在直接映射内",
            Self::UnsupportedGranularity => "实现不支持该页粒度",
        };
        f.write_str(text)
    }
}

/// 覆盖 `len` 字节所需的页数（向上取整）。
///
/// `len == 0`、`page_size == 0`、或“页数 × 页大小”溢出时返回 `None`。
pub const fn pages_for(len: u64, page_size: u64) -> Option<u64> {
    if len == 0 || page_size == 0 {
        return None;
    }
    let whole = len / page_size;
    let pages = if len % page_size == 0 {
        whole
    } else {
        match whole.checked_add(1) {
            Some(p) => p,
            None => return None,
        }
    };
    match pages.checked_mul(page_size) {
        Some(_) => Some(pages),
        None => None,
    }
}

/// 校验一个映射请求。所有实现共用（单点定义，严格模式 S15）。
pub fn validate_range(
    virt: VirtAddr,
    phys: PhysAddr,
    len: u64,
    align: Alignment,
) -> Result<(), MapError> {
    if len == 0 {
        return Err(MapError::Empty);
    }
    if !virt.is_aligned_to(align) {
        return Err(MapError::MisalignedVirt);
    }
    if !phys.is_aligned_to(align) {
        return Err(MapError::MisalignedPhys);
    }
    if len % align.as_u64() != 0 {
        return Err(MapError::MisalignedLength);
    }
    if virt.checked_add(len).is_none() || phys.checked_add(len).is_none() {
        return Err(MapError::Overflow);
    }
    Ok(())
}

/// 页帧来源：架构中立的抽象。
///
/// 任何架构都需要“取一帧已零化内存”，故它属于抽象层；
/// 实现由调用方注入（引导阶段来自固件页分配，宿主测试来自内存缓冲）。
pub trait FrameAllocator {
    /// 取一个已零化的物理页帧；耗尽返回 `None`。
    fn allocate_zeroed(&mut self) -> Option<PhysFrame>;
}

/// 页表：调用方按区间声明映射，页表层数与页大小由实现决定。
pub trait PageTable {
    /// 建立 `[virt, virt+len)` → `[phys, phys+len)` 的映射。
    fn map_range(
        &mut self,
        virt: VirtAddr,
        phys: PhysAddr,
        len: u64,
        flags: PageFlags,
    ) -> Result<(), MapError>;

    /// 激活本页表：此后 CPU 用其解析地址。
    ///
    /// # Safety
    ///
    /// 调用方必须保证新页表仍映射当前正在执行的代码与栈，否则切换后立即故障。
    unsafe fn activate(&self);
}

#[cfg(test)]
mod tests {
    use super::{FrameAllocator, MapError, PageFlags, pages_for, validate_range};
    use crate::addr::{Alignment, PhysAddr, PhysFrame, VirtAddr};


    #[test]
    fn frame_allocator_is_implementable() {
        struct Fake {
            next: u64,
        }
        impl FrameAllocator for Fake {
            fn allocate_zeroed(&mut self) -> Option<PhysFrame> {
                if self.next == 0 {
                    return None;
                }
                let frame = PhysFrame::containing(PhysAddr::new(self.next));
                self.next = 0;
                Some(frame)
            }
        }
        let mut fake = Fake { next: 0x1000 };
        assert!(fake.allocate_zeroed().is_some());
        assert!(fake.allocate_zeroed().is_none(), "耗尽后必须返回 None");
    }

    #[test]
    fn pages_for_rejects_zero_page_size_and_reports_overflow() {
        assert_eq!(pages_for(0x1000, 0), None);
        assert_eq!(pages_for(0, 0x1000), None);
        assert_eq!(pages_for(0x1000, 0x1000), Some(1));
        assert_eq!(pages_for(0x2000, 0x1000), Some(2));
        assert_eq!(pages_for(0x1001, 0x1000), Some(2), "非整倍数向上取整");
        assert_eq!(pages_for(u64::MAX, 0x1000), None, "页数计算溢出应报错");
    }


    #[test]
    fn map_error_has_human_readable_display() {
        let cases = [
            (MapError::Empty, "区间长度为零"),
            (MapError::MisalignedVirt, "虚拟地址未对齐"),
            (MapError::MisalignedPhys, "物理地址未对齐"),
            (MapError::MisalignedLength, "长度不是对齐量的整数倍"),
            (MapError::Overflow, "区间端点计算溢出"),
            (MapError::OutOfMemory, "页帧分配耗尽"),
            (MapError::TableNotAccessible, "页表帧不在直接映射内"),
            (MapError::UnsupportedGranularity, "实现不支持该页粒度"),
        ];
        for (err, want) in cases {
            assert_eq!(std::format!("{err}"), want);
        }
    }

    #[test]
    fn flags_are_semantic_bits() {
        assert!(!PageFlags::none().is_present());
        assert!(PageFlags::present().is_present());
        assert!(!PageFlags::present().is_writable());
        let rw = PageFlags::present().with(PageFlags::writable());
        assert!(rw.is_present() && rw.is_writable() && !rw.is_executable());
        assert!(PageFlags::present().with(PageFlags::executable()).is_executable());
    }

    #[test]
    fn validate_rejects_empty_range() {
        let a = Alignment::PAGE;
        assert_eq!(
            validate_range(VirtAddr::new(0x1000), PhysAddr::new(0x2000), 0, a),
            Err(MapError::Empty)
        );
    }

    #[test]
    fn validate_rejects_misaligned_inputs() {
        let a = Alignment::PAGE;
        assert_eq!(
            validate_range(VirtAddr::new(0x1001), PhysAddr::new(0x2000), 0x1000, a),
            Err(MapError::MisalignedVirt)
        );
        assert_eq!(
            validate_range(VirtAddr::new(0x1000), PhysAddr::new(0x2001), 0x1000, a),
            Err(MapError::MisalignedPhys)
        );
        assert_eq!(
            validate_range(VirtAddr::new(0x1000), PhysAddr::new(0x2000), 0x1001, a),
            Err(MapError::MisalignedLength)
        );
    }

    #[test]
    fn validate_rejects_overflow_instead_of_wrapping() {
        let a = Alignment::PAGE;
        assert_eq!(
            validate_range(VirtAddr::new(u64::MAX - 0xFFF), PhysAddr::new(0x2000), 0x1000, a),
            Err(MapError::Overflow)
        );
        assert_eq!(
            validate_range(VirtAddr::new(0x1000), PhysAddr::new(u64::MAX - 0xFFF), 0x1000, a),
            Err(MapError::Overflow)
        );
    }

    #[test]
    fn validate_accepts_aligned_range() {
        let a = Alignment::PAGE;
        assert_eq!(validate_range(VirtAddr::new(0x1000), PhysAddr::new(0x2000), 0x2000, a), Ok(()));
        assert_eq!(validate_range(VirtAddr::new(0), PhysAddr::new(0), 0x1000, a), Ok(()));
    }
}
