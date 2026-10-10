//! Kernel-owned GDT and TSS.
//!
//! The bootloader leaves its own small GDT in place and no TSS at all, so an
//! exception taken on an exhausted stack (#PF while pushing the frame) cannot
//! be delivered and escalates #PF -> #DF -> triple fault: an instant, silent
//! reset. A TSS whose IST slot 0 points at a dedicated stack lets the #DF
//! handler run on known-good memory and report the fault on COM1 instead.
//!
//! The page-fault handler gets an IST slot of its own too: a thread that runs off
//! the end of its stack hits the guard page below it (see `vm`), and the CPU must
//! push the #PF frame somewhere that is not that exhausted stack. Using a different
//! slot from #DF keeps a fault *inside* the #PF handler from being delivered on top
//! of the stack the handler is already using.
//!
//! The table also carries the user-mode segments (ring 3) for `userproc`: a data and a code
//! descriptor with DPL 3. The TSS's `RSP0` is the stack the CPU switches to when an interrupt,
//! exception or `int 0x80` arrives from ring 3; the scheduler points it at the running user
//! thread's kernel stack on every switch ([`set_rsp0`]). See docs/audit/adr-isolamento.md.

use crate::sync::RacyCell;
use core::sync::atomic::{AtomicU16, Ordering};
use x86_64::VirtAddr;
use x86_64::instructions::tables::load_tss;
use x86_64::registers::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable};
use x86_64::structures::tss::TaskStateSegment;

/// IST slot (0-based) used by the double-fault handler.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
/// IST slot (0-based) used by the page-fault handler.
pub const PAGE_FAULT_IST_INDEX: u16 = 1;

const IST_STACK_SIZE: usize = 32 * 1024;

#[repr(C, align(16))]
struct IstStack([u8; IST_STACK_SIZE]);

static IST_STACK: RacyCell<IstStack> = RacyCell::new(IstStack([0; IST_STACK_SIZE]));
static PF_IST_STACK: RacyCell<IstStack> = RacyCell::new(IstStack([0; IST_STACK_SIZE]));
static TSS: RacyCell<TaskStateSegment> = RacyCell::new(TaskStateSegment::new());
static GDT: RacyCell<GlobalDescriptorTable> = RacyCell::new(GlobalDescriptorTable::new());

static USER_DATA: AtomicU16 = AtomicU16::new(0);
static USER_CODE: AtomicU16 = AtomicU16::new(0);

/// Selector for user-mode code (RPL 3), valid after [`init`].
pub fn user_code() -> u16 {
    USER_CODE.load(Ordering::Relaxed)
}

/// Selector for user-mode data and stack (RPL 3), valid after [`init`].
pub fn user_data() -> u16 {
    USER_DATA.load(Ordering::Relaxed)
}

/// Point the stack the CPU loads on a ring 3 -> ring 0 transition at `top` (a kernel stack top).
pub fn set_rsp0(top: u64) {
    // SAFETY: the TSS is a static that only this function and `init` write; the scheduler calls it
    // with interrupts off (inside the context-switch ISR or `kill_current`), and a u64 store of an
    // aligned field is atomic on x86_64, so the CPU never sees a torn value.
    unsafe { (*TSS.get()).privilege_stack_table[0] = VirtAddr::new(top) };
}

/// Build and load the GDT/TSS and reload the segment registers.
///
/// Must run once, before interrupts are enabled and before anything captures
/// `CS`/`SS` (the IDT gates and `sched::spawn` both read the live selectors).
pub fn init() {
    // SAFETY: called once on the boot thread with interrupts still disabled, so
    // nothing else touches these statics; the statics live for 'static, which
    // `GDT.load()` and the TSS descriptor require.
    unsafe {
        let stack_top = VirtAddr::from_ptr(IST_STACK.get()) + IST_STACK_SIZE as u64;
        (*TSS.get()).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = stack_top;
        let pf_top = VirtAddr::from_ptr(PF_IST_STACK.get()) + IST_STACK_SIZE as u64;
        (*TSS.get()).interrupt_stack_table[PAGE_FAULT_IST_INDEX as usize] = pf_top;

        let gdt = &mut *GDT.get();
        let code = gdt.append(Descriptor::kernel_code_segment());
        let data = gdt.append(Descriptor::kernel_data_segment());
        let udata = gdt.append(Descriptor::user_data_segment());
        let ucode = gdt.append(Descriptor::user_code_segment());
        USER_DATA.store(udata.0, core::sync::atomic::Ordering::Relaxed);
        USER_CODE.store(ucode.0, core::sync::atomic::Ordering::Relaxed);
        let tss = gdt.append(Descriptor::tss_segment(&*TSS.get()));
        (*GDT.get()).load();

        CS::set_reg(code);
        SS::set_reg(data);
        DS::set_reg(data);
        ES::set_reg(data);
        load_tss(tss);
    }
}
