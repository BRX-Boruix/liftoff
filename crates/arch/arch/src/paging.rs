//! 分页语义接口：把"建立映射"表达为与架构无关的区间操作。
//!
//! 抽象边界（ADR-007）：调用方只声明区间与权限，页表层数与页大小由实现决定，
//! 因此本模块不出现 PML4/PD 之类的架构名词。

use crate::addr::{Alignment, PhysAddr, VirtAddr};

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
    use super::{MapError, PageFlags, validate_range};
    use crate::addr::{Alignment, PhysAddr, VirtAddr};

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
