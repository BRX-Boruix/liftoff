//! 平台无关的地址类型：字节地址与物理页帧号。
//!
//! 数值纪律（严格模式 S19）：所有可能溢出的运算返回 [`Option`]，绝不回绕；
//! 地址到 `usize` 的转换在目标指针宽度不足时返回 `None`，绝不静默截断（S04）。

/// 基础页大小（字节）。
///
/// 取值理由：x86-64 基础页为 4 KiB；Limine 协议的内存映射与栈大小语义均以字节
/// 为单位，引导器内部按页管理内存时以 4 KiB 为最小单位。
pub const PAGE_SIZE: u64 = 4096;

/// 对齐量（字节）。
///
/// 不变量：取值恒为 2 的幂且非零——只能经 [`Alignment::new_power_of_two`] 构造，
/// 因此对齐运算不可能收到非法对齐量（严格模式：宁可报错，不靠文档前置条件）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Alignment(u64);

impl Alignment {
    /// 基础页对齐量（4 KiB），供页级操作使用。
    pub const PAGE: Alignment = Alignment(PAGE_SIZE);

    /// 由 2 的幂构造；`0` 与非 2 的幂返回 `None`。
    #[inline]
    pub const fn new_power_of_two(bytes: u64) -> Option<Self> {
        if bytes != 0 && bytes.is_power_of_two() {
            Some(Self(bytes))
        } else {
            None
        }
    }

    /// 对齐量（字节）。
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

macro_rules! byte_address {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
        pub struct $name(u64);

        impl $name {
            /// 由字节值构造。
            #[inline]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// 取原始字节值。
            #[inline]
            pub const fn as_u64(self) -> u64 {
                self.0
            }

            /// 转为 `usize`；目标指针宽度不足时返回 `None`（绝不静默截断）。
            #[inline]
            pub const fn as_usize(self) -> Option<usize> {
                #[cfg(target_pointer_width = "64")]
                {
                    Some(self.0 as usize)
                }
                #[cfg(not(target_pointer_width = "64"))]
                {
                    if self.0 <= usize::MAX as u64 {
                        Some(self.0 as usize)
                    } else {
                        None
                    }
                }
            }

            /// 是否按 `align` 对齐。
            #[inline]
            pub const fn is_aligned_to(self, align: Alignment) -> bool {
                self.0 % align.0 == 0
            }

            /// 向下对齐。
            #[inline]
            pub const fn align_down(self, align: Alignment) -> Self {
                Self(self.0 & !(align.0 - 1))
            }

            /// 向上对齐；溢出时返回 `None`。
            #[inline]
            pub const fn align_up(self, align: Alignment) -> Option<Self> {
                match self.0.checked_add(align.0 - 1) {
                    Some(sum) => Some(Self(sum & !(align.0 - 1))),
                    None => None,
                }
            }

            /// 加法；溢出时返回 `None`。
            #[inline]
            pub const fn checked_add(self, bytes: u64) -> Option<Self> {
                match self.0.checked_add(bytes) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }

            /// 减法；下溢时返回 `None`。
            #[inline]
            pub const fn checked_sub(self, bytes: u64) -> Option<Self> {
                match self.0.checked_sub(bytes) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }
        }
    };
}

byte_address!(PhysAddr, "物理地址（字节）。");
byte_address!(VirtAddr, "虚拟地址（字节）。");

/// 物理页帧号（以 [`PAGE_SIZE`] 为单位）。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct PhysFrame(u64);

impl PhysFrame {
    /// 包含给定物理地址的页帧。
    #[inline]
    pub const fn containing(address: PhysAddr) -> Self {
        Self(address.as_u64() / PAGE_SIZE)
    }

    /// 由帧号构造。
    #[inline]
    pub const fn from_index(index: u64) -> Self {
        Self(index)
    }

    /// 帧号。
    #[inline]
    pub const fn index(self) -> u64 {
        self.0
    }

    /// 帧的起始物理地址；帧号乘页大小溢出时返回 `None`。
    #[inline]
    pub const fn start_address(self) -> Option<PhysAddr> {
        match self.0.checked_mul(PAGE_SIZE) {
            Some(base) => Some(PhysAddr::new(base)),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Alignment, PhysAddr, PhysFrame, VirtAddr, PAGE_SIZE};

    #[test]
    fn align_down_rounds_towards_zero() {
        assert_eq!(PhysAddr::new(0x1234).align_down(Alignment::PAGE).as_u64(), 0x1000);
        assert_eq!(PhysAddr::new(0x1000).align_down(Alignment::PAGE).as_u64(), 0x1000);
        assert_eq!(PhysAddr::new(0).align_down(Alignment::PAGE).as_u64(), 0);
    }

    #[test]
    fn align_up_keeps_aligned_values_unchanged() {
        assert_eq!(PhysAddr::new(0x1000).align_up(Alignment::PAGE).map(|a| a.as_u64()), Some(0x1000));
        assert_eq!(PhysAddr::new(0x1001).align_up(Alignment::PAGE).map(|a| a.as_u64()), Some(0x2000));
        assert_eq!(PhysAddr::new(0).align_up(Alignment::PAGE).map(|a| a.as_u64()), Some(0));
    }

    #[test]
    fn align_up_reports_overflow_instead_of_wrapping() {
        assert_eq!(PhysAddr::new(u64::MAX).align_up(Alignment::PAGE), None);
        assert_eq!(VirtAddr::new(u64::MAX).align_up(Alignment::PAGE), None);
    }

    #[test]
    fn checked_arithmetic_reports_overflow() {
        assert_eq!(PhysAddr::new(u64::MAX).checked_add(1), None);
        assert_eq!(PhysAddr::new(0).checked_sub(1), None);
        assert_eq!(PhysAddr::new(0x10).checked_add(0x10).map(|a| a.as_u64()), Some(0x20));
    }

    #[test]
    fn page_frame_maps_to_its_first_address() {
        assert_eq!(PhysFrame::containing(PhysAddr::new(0x1FFF)).index(), 1);
        assert_eq!(PhysFrame::containing(PhysAddr::new(0x1FFF)).start_address().map(|a| a.as_u64()), Some(0x1000));
        assert_eq!(PhysFrame::from_index(2).start_address().map(|a| a.as_u64()), Some(0x2000));
    }

    #[test]
    fn page_frame_start_address_reports_overflow() {
        assert_eq!(PhysFrame::from_index(u64::MAX).start_address(), None);
    }

    #[test]
    fn as_usize_never_truncates_silently() {
        assert_eq!(PhysAddr::new(0x1234).as_usize(), Some(0x1234));
        if core::mem::size_of::<usize>() < 8 {
            assert_eq!(PhysAddr::new(u64::MAX).as_usize(), None);
        }
    }

    #[test]
    fn alignment_rejects_non_power_of_two_and_zero() {
        assert_eq!(Alignment::new_power_of_two(0), None);
        assert_eq!(Alignment::new_power_of_two(3), None);
        assert_eq!(Alignment::new_power_of_two(4096).map(|a| a.as_u64()), Some(4096));
        assert_eq!(Alignment::PAGE.as_u64(), PAGE_SIZE);
    }

    #[test]
    fn align_ops_use_the_alignment_invariant() {
        assert!(PhysAddr::new(0x2000).is_aligned_to(Alignment::PAGE));
        assert!(!PhysAddr::new(0x2001).is_aligned_to(Alignment::PAGE));
        assert_eq!(PhysAddr::new(0x2001).align_up(Alignment::PAGE).map(|a| a.as_u64()), Some(0x3000));
    }
}
