//! ELF64 静态装载器（M3）。布局依据：System V ABI x86-64 §ELF + 内核实测形态。
//!
//! 装载模型（brxLimine common/lib/elf.c 916..1039 同构，去 KASLR 与重定位——
//! 那是 M4 Limine/BORUIX 协议层的职责）：
//!  1. 全部 PT_LOAD（memsz>0）参与布局；min_vaddr = 最小 p_vaddr，
//!     image_size = max(p_vaddr+p_memsz) - min_vaddr（vaddr 跨度，含段间空洞）；
//!  2. 物理映像一次连续分配（AllocateAnyPages，UEFI §7.2 保证单次调用返回
//!     连续页块），基址 image_base；
//!  3. 段落位：dst = image_base + (p_vaddr - min_vaddr)，复制 p_filesz 字节，
//!     [filesz, memsz) 清零（BSS 语义，brxLimine elf.c 1035..1039 同款）；
//!  4. 校验：p_filesz<=p_memsz、offset+filesz 不越文件、页重叠拒绝
//!     （不同权限段共享 4KB 页在 M3 无页表保护时是真实风险，一律拒绝）。
//!
//! ET_DYN（PIE）接受但**不做重定位**：内核是 -2GB 高链接 PIE，其重定位
//!（R_X86_64_RELATIVE）按物理基址锚定，M4 协议层交接物理基址后由内核自重定位
//! 或由协议层完成——当前装载结果只用于验收链路验证，不用于实际跳转。

use crate::efi;

// ---------------------------------------------------------------- ELF 常量

/// "\x7fELF" 魔数。
pub const ELF_MAGIC: [u8; 4] = [0x7F, 0x45, 0x4C, 0x46];

/// ELFCLASS64（e_ident[EI_CLASS]，§e_ident 表）。
pub const ELF_CLASS64: u8 = 2;

/// ELFDATA2LSB（e_ident[EI_DATA]，小端）。
pub const ELF_DATA_LSB: u8 = 1;

/// ET_EXEC（可执行，e_type）。
pub const ET_EXEC: u16 = 2;

/// ET_DYN（PIE/共享对象，e_type）。
pub const ET_DYN: u16 = 3;

/// EM_X86_64（e_machine）。
pub const EM_X86_64: u16 = 62;

/// PT_LOAD（p_type）。
pub const PT_LOAD: u32 = 1;

/// ELF64 程序头大小（Elf64_Phdr，56 字节）。
pub const PHDR_SIZE: usize = 56;

/// 内核装载页粒度（4KB；PHDR 重叠检查的页宽）。
pub const PAGE_SIZE: u64 = 4096;

// ---------------------------------------------------------------- 错误码

/// 内部错误码（串口以 16 进制直报；0x30.. 段为 ELF 域）。
pub mod err {
    pub const BAD_MAGIC: usize = 0x30;
    pub const BAD_CLASS: usize = 0x31;
    pub const BAD_ENDIAN: usize = 0x32;
    pub const BAD_MACHINE: usize = 0x33;
    pub const BAD_TYPE: usize = 0x34;
    pub const BAD_PHDR: usize = 0x35;
    pub const SEG_TOO_BIG: usize = 0x36;
    pub const NO_LOAD: usize = 0x37;
    /// PIE 重定位处理失败（无 PT_DYNAMIC/DT_RELA 越界/类型非 RELATIVE）。
    pub const NO_RELA: usize = 0x3A;
    pub const OVERLAP: usize = 0x38;
    pub const ALLOC_FAIL: usize = 0x39;
}

// ---------------------------------------------------------------- 盘上结构

/// 程序头切片。
pub struct ProgHeader {
    pub p_type: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
}

/// 解析一个程序头（读取并校验在 load 内联进行；这里只做字节切片）。
fn parse_phdr(buf: &[u8], off: usize) -> Result<ProgHeader, usize> {
    if off + PHDR_SIZE > buf.len() {
        return Err(err::BAD_PHDR);
    }
    let le32 = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    let le64 = |o: usize| {
        u64::from_le_bytes([
            buf[o], buf[o + 1], buf[o + 2], buf[o + 3],
            buf[o + 4], buf[o + 5], buf[o + 6], buf[o + 7],
        ])
    };
    Ok(ProgHeader {
        p_type: le32(off),
        p_offset: le64(off + 8),
        p_vaddr: le64(off + 16),
        p_filesz: le64(off + 32),
        p_memsz: le64(off + 40),
    })
}

// ---------------------------------------------------------------- 装载结果

/// 段报告（串口契约行的数据源）。
#[derive(Clone, Copy)]
pub struct SegReport {
    pub rel: u64,
    pub memsz: u64,
    pub sum16: u16,
}

/// 装载结果：物理落位与全部报告面。
pub struct LoadResult {
    pub image_base: u64,
    pub image_size: u64,
    pub vbase: u64,
    pub entry: u64,
    pub segs: usize,
    pub total_mem: u64,
    pub total_sum16: u16,
    pub reports: [SegReport; 8],
}

// ---------------------------------------------------------------- 装载器

/// 从完整 ELF 文件缓冲装载。
pub fn load(blob: &[u8], bs: &efi::BootServices) -> Result<LoadResult, usize> {
    if blob.len() < 64 || blob[..4] != ELF_MAGIC {
        return Err(err::BAD_MAGIC);
    }
    if blob[4] != ELF_CLASS64 {
        return Err(err::BAD_CLASS);
    }
    if blob[5] != ELF_DATA_LSB {
        return Err(err::BAD_ENDIAN);
    }
    let e_type = u16::from_le_bytes([blob[16], blob[17]]);
    if e_type != ET_EXEC && e_type != ET_DYN {
        return Err(err::BAD_TYPE);
    }
    let e_machine = u16::from_le_bytes([blob[18], blob[19]]);
    if e_machine != EM_X86_64 {
        return Err(err::BAD_MACHINE);
    }
    let e_entry = u64::from_le_bytes([
        blob[24], blob[25], blob[26], blob[27], blob[28], blob[29], blob[30], blob[31],
    ]);
    let e_phoff = u64::from_le_bytes([
        blob[32], blob[33], blob[34], blob[35], blob[36], blob[37], blob[38], blob[39],
    ]);
    let e_phentsize = u16::from_le_bytes([blob[54], blob[55]]);
    let e_phnum = u16::from_le_bytes([blob[56], blob[57]]);
    if e_phentsize as usize != PHDR_SIZE || e_phnum == 0 {
        return Err(err::BAD_PHDR);
    }

    // 第一遍：收集 PT_LOAD，校验，算布局。
    let mut min_vaddr = u64::MAX;
    let mut max_end: u64 = 0;
    let mut loads: [ProgHeader; 8] = core::array::from_fn(|_| ProgHeader {
        p_type: 0,
        p_offset: 0,
        p_vaddr: 0,
        p_filesz: 0,
        p_memsz: 0,
    });
    let mut n_loads = 0usize;
    for i in 0..e_phnum as usize {
        let ph = parse_phdr(blob, e_phoff as usize + i * PHDR_SIZE)?;
        if ph.p_type != PT_LOAD || ph.p_memsz == 0 {
            continue;
        }
        if ph.p_filesz > ph.p_memsz {
            return Err(err::BAD_PHDR);
        }
        if ph.p_offset.checked_add(ph.p_filesz).map_or(true, |end| end > blob.len() as u64) {
            return Err(err::SEG_TOO_BIG);
        }
        if ph.p_vaddr.checked_add(ph.p_memsz).is_none() {
            return Err(err::BAD_PHDR);
        }
        // 页重叠拒绝（brxLimine elf.c 951..959：不同权限段共享 4KB 页不可接受；
        // M3 无页表，页内不同权限无法区分，同页段一律拒绝）。
        let page_lo = ph.p_vaddr & !(PAGE_SIZE - 1);
        let page_hi = (ph.p_vaddr + ph.p_memsz + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        for j in 0..n_loads {
            let o = &loads[j];
            let o_lo = o.p_vaddr & !(PAGE_SIZE - 1);
            let o_hi = (o.p_vaddr + o.p_memsz + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            if page_lo < o_hi && o_lo < page_hi {
                return Err(err::OVERLAP);
            }
        }
        if ph.p_vaddr < min_vaddr {
            min_vaddr = ph.p_vaddr;
        }
        if ph.p_vaddr + ph.p_memsz > max_end {
            max_end = ph.p_vaddr + ph.p_memsz;
        }
        if n_loads >= 8 {
            return Err(err::NO_LOAD); // 上限：内核 3..4 段，8 已留余量
        }
        loads[n_loads] = ph;
        n_loads += 1;
    }
    if n_loads == 0 {
        return Err(err::NO_LOAD);
    }

    let image_size = max_end - min_vaddr;
    let pages = ((image_size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
    // 2MB 对齐预留：多分配 512 页，取块内 2MB 对齐子区（大页要求物理 2MB 对齐，
    // AllocateAnyPages 只保证 4KiB 对齐——不对齐会让切 CR3 后首个内核页取指 #PF）。
    const HUGE_ALIGN_PAGES: usize = 512; // 512*4KiB = 2MiB
    let alloc_pages = pages + HUGE_ALIGN_PAGES;
    let mut alloc_base: u64 = 0;
    let status = unsafe {
        (bs.allocate_pages)(
            efi::ALLOCATE_ANY_PAGES,
            efi::MEMORY_LOADER_DATA,
            alloc_pages,
            &mut alloc_base,
        )
    };
    if efi::is_error(status) {
        return Err(err::ALLOC_FAIL);
    }
    let image_base = (alloc_base + 0x1F_FFFF) & !0x1F_FFFF;

    // 映像区整体清零（空洞与 BSS 同源，杜绝陈旧内存泄漏进内核）。
    let image = unsafe {
        core::slice::from_raw_parts_mut(image_base as *mut u8, image_size as usize)
    };
    image.fill(0);

    // 第二遍：落位。
    let mut total_mem: u64 = 0;
    let mut total_sum: u32 = 0;
    let mut reports = [SegReport { rel: 0, memsz: 0, sum16: 0 }; 8];
    for i in 0..n_loads {
        let ph = &loads[i];
        let rel = ph.p_vaddr - min_vaddr;
        let dst = &mut image[rel as usize..(rel + ph.p_memsz) as usize];
        let src = &blob[ph.p_offset as usize..(ph.p_offset + ph.p_filesz) as usize];
        dst[..ph.p_filesz as usize].copy_from_slice(src);
        let mut sum: u32 = 0;
        for &b in src {
            sum = sum.wrapping_add(b as u32);
        }
        reports[i] = SegReport { rel, memsz: ph.p_memsz, sum16: sum as u16 };
        total_mem += ph.p_memsz;
        total_sum = total_sum.wrapping_add(sum);
    }


    // 第三遍：PIE 重定位（R_X86_64_RELATIVE）。内核是 DYN/PIE（readelf Type:
    // DYN），.rela.dyn 3138 条 RELATIVE——GOT/字面量槽在文件里为 0，约定由
    // loader 填（brxLimine elf.c 916..1039 同款）。不做此步，kmain 的
    // `mov rsp,[rip+..]` 读 __kstack_top 得 0 → rsp=0 → push #PF
    // （CR2=0xfffffffffffffff8，QMP 抓拍实锤）。
    // 高半链接（vbase=0xffffffff80000000）恒等装载时 load_bias=0，
    // 槽值 = addend（即链接期符号虚地址）。
    if e_type == ET_DYN {
        const PT_DYNAMIC: u32 = 2;
        const DT_NULL: u64 = 0;
        const DT_RELA: u64 = 7;
        const DT_RELASZ: u64 = 8;
        const DT_RELAENT: u64 = 9;
        const R_X86_64_RELATIVE: u64 = 8;
        let mut dyn_va: u64 = 0;
        let mut dyn_sz: u64 = 0;
        for i in 0..e_phnum as usize {
            let ph = parse_phdr(blob, e_phoff as usize + i * PHDR_SIZE)?;
            if ph.p_type == PT_DYNAMIC {
                dyn_va = ph.p_vaddr;
                dyn_sz = ph.p_filesz;
                break;
            }
        }
        if dyn_va == 0 || dyn_sz == 0 {
            return Err(err::NO_RELA);
        }
        let mut dyn_rel: u64 = 0;
        let mut found = false;
        for i in 0..n_loads {
            let ph = &loads[i];
            if dyn_va >= ph.p_vaddr && dyn_va + dyn_sz <= ph.p_vaddr + ph.p_memsz {
                dyn_rel = dyn_va - min_vaddr;
                found = true;
                break;
            }
        }
        if !found {
            return Err(err::NO_RELA);
        }
        // 3b) 解析 dynamic 数组（16B/项：tag+val）。
        let dynb = &image[dyn_rel as usize..(dyn_rel + dyn_sz) as usize];
        let mut rela_va: u64 = 0;
        let mut relasz: u64 = 0;
        let mut relaent: u64 = 0;
        let mut k = 0usize;
        while k + 16 <= dynb.len() {
            let tag = u64::from_le_bytes(dynb[k..k + 8].try_into().unwrap());
            if tag == DT_NULL {
                break;
            }
            let val = u64::from_le_bytes(dynb[k + 8..k + 16].try_into().unwrap());
            if tag == DT_RELA {
                rela_va = val;
            } else if tag == DT_RELASZ {
                relasz = val;
            } else if tag == DT_RELAENT {
                relaent = val;
            }
            k += 16;
        }
        if rela_va == 0 || relasz == 0 || relaent != 24 {
            return Err(err::NO_RELA);
        }
        // rela_va（链接 va）→ image 内偏移。
        let mut rela_rel: u64 = 0;
        let mut found = false;
        for i in 0..n_loads {
            let ph = &loads[i];
            if rela_va >= ph.p_vaddr && rela_va + relasz <= ph.p_vaddr + ph.p_memsz {
                rela_rel = rela_va - min_vaddr;
                found = true;
                break;
            }
        }
        if !found {
            return Err(err::NO_RELA);
        }
        // 3c) 逐条 RELATIVE：slot(image_base+r_offset-vbase) = addend。
        // relab 借用 image（不可变）与写回冲突——全部走裸指针（单线程引导期，
        // rela 区与 slot 区可能重叠无法拆借用）。
        let image_ptr = image.as_mut_ptr();
        let relab = unsafe {
            core::slice::from_raw_parts(image_ptr.add(rela_rel as usize), relasz as usize)
        };
        let mut o = 0usize;
        while o + 24 <= relab.len() {
            let r_offset = u64::from_le_bytes(relab[o..o + 8].try_into().unwrap());
            let r_info = u64::from_le_bytes(relab[o + 8..o + 16].try_into().unwrap());
            if r_info & 0xFFFF_FFFF == R_X86_64_RELATIVE {
                let addend = u64::from_le_bytes(relab[o + 16..o + 24].try_into().unwrap());
                if r_offset < min_vaddr {
                    return Err(err::NO_RELA);
                }
                let slot_rel = r_offset - min_vaddr;
                if slot_rel + 8 > image.len() as u64 {
                    return Err(err::NO_RELA);
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        addend.to_le_bytes().as_ptr(),
                        image_ptr.add(slot_rel as usize),
                        8,
                    );
                }
            }
            o += relaent as usize;
        }
    }
    Ok(LoadResult {
        image_base,
        image_size,
        vbase: min_vaddr,
        entry: e_entry,
        segs: n_loads,
        total_mem,
        total_sum16: total_sum as u16,
        reports,
    })
}