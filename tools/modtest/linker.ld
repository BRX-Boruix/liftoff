/* modtest 链接脚本：高半静态布局，两段（RE / RW），页对齐避免段重叠拒绝。
   与 BORUIX 内核同款高半 vbase，确保走 liftoff 的内核高区映射路径。 */
ENTRY(kmain)

SECTIONS
{
    . = 0xffffffff80000000;

    .text : ALIGN(4096) {
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096) {
        *(.data .data.*)
        *(.limine_reqs)
    }

    .bss : ALIGN(4096) {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.comment)
    }
}
