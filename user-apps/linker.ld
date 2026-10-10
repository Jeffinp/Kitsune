/* Layout of a Kitsune user program: one page-aligned segment per kind of memory, so the loader can
   map code read-execute, constants read-only and data read-write-no-execute with no page shared. */
ENTRY(_start)

SECTIONS {
    . = 0x400000;

    .text : ALIGN(4096) { *(.text .text.*) }
    .rodata : ALIGN(4096) { *(.rodata .rodata.*) }
    .data : ALIGN(4096) { *(.data .data.*) }
    .bss : ALIGN(4096) { *(.bss .bss.*) *(COMMON) }

    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) *(.gnu*) }
}
