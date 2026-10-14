//! IDT, CPU exception handlers, 8259 PIC remap, 8253/8254 PIT timer, and
//! IRQ-driven PS/2 input.
//!
//! The keyboard (IRQ1) and mouse (IRQ12) ISRs push tagged bytes into a
//! single-producer/single-consumer ring buffer that the main loop drains, so
//! no busy-polling is needed. Fatal CPU exceptions are reported on COM1 and on
//! screen (see `crash`) instead of triple-faulting (silent reboot) or freezing
//! without a word.

use crate::io::{inb, outb};
use crate::sync::RacyCell;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use x86_64::VirtAddr;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

/// Monotonic timer tick count (incremented at `TIMER_HZ`).
pub static TICKS: AtomicU64 = AtomicU64::new(0);
/// Timer / scheduler quantum frequency. Higher = smoother time-slicing.
pub const TIMER_HZ: u32 = 250;
/// While a video plays the PIT fires this many times per tick ([`set_fast`]): the extra interrupts
/// ("sub-ticks", 1 ms apart) do not count as ticks and do not end a thread's slice; they only let
/// the compositor put a picture on screen within a millisecond of its time (`sched::present`).
pub const SUB_PER_TICK: u32 = 4;
/// The PIT runs at `TIMER_HZ * SUB_PER_TICK`.
static FAST: AtomicBool = AtomicBool::new(false);
/// Interrupts of the PIT since it went fast (a tick is every `SUB_PER_TICK`-th).
static SUB: AtomicU32 = AtomicU32::new(0);

/// Tags on bytes in the input ring (high byte of each entry).
pub const SRC_KEYBOARD: u16 = 0;
pub const SRC_MOUSE: u16 = 1;

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;
const PIC_EOI: u8 = 0x20;
const PS2_DATA: u16 = 0x60;

const TIMER_VECTOR: u8 = 32; // IRQ0
const KEYBOARD_VECTOR: u8 = 33; // IRQ1
const MOUSE_VECTOR: u8 = 44; // IRQ12 (slave PIC)
/// Software interrupt a blocking thread raises (`int 0x81`) to hand over the CPU.
pub const YIELD_VECTOR: u8 = 0x81;
/// The system-call vector: a gate ring 3 may use (DPL 3), unlike every other vector here.
pub const SYSCALL_VECTOR: u8 = 0x80;
const SPURIOUS_MASTER_VECTOR: u8 = 39; // IRQ7 (master PIC)
const SPURIOUS_SLAVE_VECTOR: u8 = 47; // IRQ15 (slave PIC)

/// OCW3: make the next read of the command port return the In-Service Register.
const PIC_READ_ISR: u8 = 0x0B;
/// OCW3: make the next read of the command port return the Interrupt Request
/// Register (the power-on default).
const PIC_READ_IRR: u8 = 0x0A;

/// Spurious IRQ7 / IRQ15 seen so far (index 0 = master, 1 = slave).
pub static SPURIOUS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

static IDT: RacyCell<InterruptDescriptorTable> = RacyCell::new(InterruptDescriptorTable::new());

pub fn init() {
    // SAFETY: boot-only: called once from `kernel_main` with IF still clear (`enable()` is below),
    // so IDT is not accessed concurrently, and nothing touches it after `load`. `set_stack_index`
    // needs a valid IST slot: `gdt::init` (run earlier) put one in the TSS. `set_handler_addr`
    // needs a real ISR: `timer_isr` saves/restores all GPRs and ends in `iretq`.
    unsafe {
        let idt = &mut *IDT.get();
        idt.breakpoint.set_handler_fn(breakpoint);
        idt.general_protection_fault
            .set_handler_fn(general_protection);
        // On its own IST stack: a stack overflow faults on the guard page with `rsp` still at the end of
        // the exhausted stack, where the CPU could not push the exception frame.
        idt.page_fault
            .set_handler_fn(page_fault)
            .set_stack_index(crate::gdt::PAGE_FAULT_IST_INDEX);
        idt.double_fault
            .set_handler_fn(double_fault)
            .set_stack_index(crate::gdt::DOUBLE_FAULT_IST_INDEX);
        // Without these, #DE/#UD/#NP/#SS escalate to a silent #DF.
        idt.divide_error.set_handler_fn(divide_error);
        idt.invalid_opcode.set_handler_fn(invalid_opcode);
        idt.segment_not_present.set_handler_fn(segment_not_present);
        idt.stack_segment_fault.set_handler_fn(stack_segment_fault);
        // The remaining architectural exceptions have no recovery path either; give each a
        // handler that reports and halts instead of letting it escalate to #DF.
        idt.debug.set_handler_fn(debug_exception);
        idt.non_maskable_interrupt.set_handler_fn(nmi);
        idt.overflow.set_handler_fn(overflow);
        idt.bound_range_exceeded.set_handler_fn(bound_range);
        idt.device_not_available
            .set_handler_fn(device_not_available);
        idt.invalid_tss.set_handler_fn(invalid_tss);
        idt.x87_floating_point.set_handler_fn(x87_floating_point);
        idt.alignment_check.set_handler_fn(alignment_check);
        idt.machine_check.set_handler_fn(machine_check);
        idt.simd_floating_point.set_handler_fn(simd_floating_point);
        idt.virtualization.set_handler_fn(virtualization);
        idt.cp_protection_exception.set_handler_fn(cp_protection);
        idt.hv_injection_exception.set_handler_fn(hv_injection);
        idt.vmm_communication_exception
            .set_handler_fn(vmm_communication);
        idt.security_exception.set_handler_fn(security_exception);
        // The timer uses a naked ISR that performs a full preemptive context
        // switch, so it's installed by raw address instead of `set_handler_fn`.
        idt[TIMER_VECTOR].set_handler_addr(VirtAddr::from_ptr(timer_isr as *const ()));
        idt[YIELD_VECTOR].set_handler_addr(VirtAddr::from_ptr(yield_isr as *const ()));
        idt[SYSCALL_VECTOR]
            .set_handler_addr(VirtAddr::from_ptr(syscall_isr as *const ()))
            .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        idt[KEYBOARD_VECTOR].set_handler_fn(keyboard);
        idt[MOUSE_VECTOR].set_handler_fn(mouse);
        // Spurious IRQ7/IRQ15 (an IRQ line that dropped before the PIC was acknowledged).
        idt[SPURIOUS_MASTER_VECTOR].set_handler_fn(spurious_master);
        idt[SPURIOUS_SLAVE_VECTOR].set_handler_fn(spurious_slave);
        // `load` needs `&'static self`; the table lives in a `static`, so a
        // reference derived from its raw pointer is genuinely `'static`.
        (*IDT.get()).load();
    }
    remap_pic();
    init_pit(TIMER_HZ);
    x86_64::instructions::interrupts::enable();
}

// ---- input ring (SPSC: ISR produces, main loop consumes) ----

const RING_CAP: usize = 512;

struct InputRing {
    buf: UnsafeCell<[u16; RING_CAP]>,
    head: AtomicUsize, // consumer
    tail: AtomicUsize, // producer
}

// Safe: ISRs run with interrupts disabled (no producer re-entrancy) and the
// main loop is the only consumer; head/tail use acquire/release ordering.
// SAFETY: SPSC. The only producer is `ring_push`, called from the keyboard/mouse ISRs (IF=0,
// so on one core they never nest); the only consumer is `read_input`, called by the compositor
// via `ps2::poll`. A slot is written before `tail` is published (Release) and read after it is
// seen (Acquire); the full check keeps the producer off the consumer's slot.
unsafe impl Sync for InputRing {}

static RING: InputRing = InputRing {
    buf: UnsafeCell::new([0; RING_CAP]),
    head: AtomicUsize::new(0),
    tail: AtomicUsize::new(0),
};

fn ring_push(value: u16) {
    let tail = RING.tail.load(Ordering::Relaxed);
    let next = (tail + 1) % RING_CAP;
    if next == RING.head.load(Ordering::Acquire) {
        return; // full: drop the byte
    }
    // SAFETY: `tail < RING_CAP` (stored values are reduced mod RING_CAP) and slot `tail` is not
    // the consumer's (`next != head` was checked); only the ISR (IF=0) writes, and the Release
    // store below publishes it.
    unsafe { (*RING.buf.get())[tail] = value };
    RING.tail.store(next, Ordering::Release);
}

/// Current monotonic timer tick.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// True if the input ring holds an unread byte (the compositor's "work to do"
/// test before it idles).
pub fn input_pending() -> bool {
    RING.head.load(Ordering::Relaxed) != RING.tail.load(Ordering::Acquire)
}

/// Pop one tagged input byte (`SRC_* << 8 | byte`), or `None` if empty.
pub fn read_input() -> Option<u16> {
    let head = RING.head.load(Ordering::Relaxed);
    if head == RING.tail.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: `head < RING_CAP`; `head != tail` (Acquire) means the producer already published
    // this slot and will not reuse it until `head` advances; single consumer (compositor).
    let value = unsafe { (*RING.buf.get())[head] };
    RING.head.store((head + 1) % RING_CAP, Ordering::Release);
    Some(value)
}

// ---- ISRs ----

// The preemptive context switch is hand-written x86_64 assembly in `switch.s`,
// assembled here via `global_asm!`. It defines the `timer_isr` symbol installed
// in the IDT for IRQ0 and tail-calls `timer_schedule` (the Rust half below).
core::arch::global_asm!(
    include_str!("switch.s"),
    schedule = sym timer_schedule,
    yield_schedule = sym yield_schedule,
    syscall_schedule = sym crate::userproc::syscall_entry,
);

unsafe extern "C" {
    /// Naked timer ISR entry (see `switch.s`): saves the interrupted context,
    /// switches stacks to the next thread, and `iretq`s into it.
    fn timer_isr();
    /// Same context-switch body on the yield vector (see `switch.s`).
    fn yield_isr();
    /// The `int 0x80` system-call entry for user processes (see `switch.s`).
    fn syscall_isr();
}

/// Rust half of the timer ISR: advance the clock, round-robin to the next
/// thread, and acknowledge the interrupt. Returns the next thread's `rsp`.
extern "C" fn timer_schedule(rsp: u64) -> u64 {
    if FAST.load(Ordering::Relaxed)
        && !(SUB.fetch_add(1, Ordering::Relaxed) + 1).is_multiple_of(SUB_PER_TICK)
    {
        // A sub-tick: no tick, no time slice ends; only a picture that is due brings the compositor.
        let next = crate::sched::sub_tick_switch(rsp);
        outb(PIC1_CMD, PIC_EOI);
        return next;
    }
    // Entropy: one TSC read + one atomic store into a static ring (no allocation, no lock).
    crate::rng::sample(crate::rng::TIMER);
    let t0 = crate::trace::t();
    TICKS.fetch_add(1, Ordering::Relaxed);
    crate::trace::timer_sample(rsp, crate::sched::current());
    let next = crate::sched::switch_current(rsp);
    outb(PIC1_CMD, PIC_EOI); // EOI before iretq re-enables interrupts
    if crate::trace::ON {
        let d = crate::io::rdtsc().wrapping_sub(t0);
        crate::trace::ISR_N.fetch_add(1, Ordering::Relaxed);
        crate::trace::ISR_CYC.fetch_add(d, Ordering::Relaxed);
        crate::trace::max_to(&crate::trace::ISR_MAX, d);
    }
    next
}

/// Rust half of the yield interrupt: pick the next runnable thread without
/// touching the clock or the PIC (this is a software interrupt, not IRQ0).
extern "C" fn yield_schedule(rsp: u64) -> u64 {
    crate::sched::yield_switch(rsp)
}

extern "x86-interrupt" fn keyboard(_f: InterruptStackFrame) {
    let byte = inb(PS2_DATA);
    // Entropy: key timing; one TSC read + one atomic store (no allocation, no lock).
    crate::rng::sample(crate::rng::KEYBOARD);
    crate::trace::input_irq();
    ring_push((SRC_KEYBOARD << 8) | byte as u16);
    outb(PIC1_CMD, PIC_EOI);
}

extern "x86-interrupt" fn mouse(_f: InterruptStackFrame) {
    let byte = inb(PS2_DATA);
    // Entropy: mouse packet timing; one TSC read + one atomic store (no allocation, no lock).
    crate::rng::sample(crate::rng::MOUSE);
    crate::trace::input_irq();
    ring_push((SRC_MOUSE << 8) | byte as u16);
    // IRQ12 is on the slave PIC: EOI to both slave and master.
    outb(PIC2_CMD, PIC_EOI);
    outb(PIC1_CMD, PIC_EOI);
}

extern "x86-interrupt" fn breakpoint(_f: InterruptStackFrame) {}

/// Handle a CPU exception raised by software (#GP, #PF, #UD, ...): if it happened in
/// a plain thread context it kills only that thread, otherwise it is fatal. In the
/// fatal case it reports on COM1 and on screen, using no allocation or locks; see
/// `crash::fault` and `crash::die`.
fn fatal(name: &str, f: &InterruptStackFrame, code: Option<u64>) -> ! {
    fatal_msg(name, f, code, format_args!("unrecoverable CPU exception"))
}

fn fatal_msg(
    name: &str,
    f: &InterruptStackFrame,
    code: Option<u64>,
    msg: core::fmt::Arguments<'_>,
) -> ! {
    crate::crash::fault(
        crate::crash::Kind::Exception,
        name,
        msg,
        Some(frame_of(f, code)),
        f.cpu_flags
            .contains(x86_64::registers::rflags::RFlags::INTERRUPT_FLAG),
    )
}

/// An exception that is never contained to a thread: a double fault, NMI, machine check or
/// anything that signals broken hardware or a broken kernel rather than a buggy thread.
fn fatal_always(name: &str, f: &InterruptStackFrame, code: Option<u64>) -> ! {
    fatal_always_msg(name, f, code, format_args!("unrecoverable CPU exception"))
}

fn fatal_always_msg(
    name: &str,
    f: &InterruptStackFrame,
    code: Option<u64>,
    msg: core::fmt::Arguments<'_>,
) -> ! {
    crate::crash::die(
        crate::crash::Kind::Exception,
        name,
        msg,
        Some(frame_of(f, code)),
    )
}

fn frame_of(f: &InterruptStackFrame, code: Option<u64>) -> crate::crash::Frame {
    crate::crash::Frame {
        rip: f.instruction_pointer.as_u64(),
        rsp: f.stack_pointer.as_u64(),
        rflags: f.cpu_flags.bits(),
        code,
    }
}

extern "x86-interrupt" fn double_fault(f: InterruptStackFrame, code: u64) -> ! {
    fatal_always_msg(
        "#DF double fault",
        &f,
        Some(code),
        format_args!("a fault occurred while delivering another one (stack overflow?)"),
    )
}

extern "x86-interrupt" fn general_protection(f: InterruptStackFrame, code: u64) {
    fatal("#GP general protection", &f, Some(code))
}

extern "x86-interrupt" fn page_fault(f: InterruptStackFrame, code: PageFaultErrorCode) {
    let cr2 = x86_64::registers::control::Cr2::read_raw();
    // A hit on a thread's guard page is a stack overflow (that thread's own, normally).
    match crate::sched::guard_owner(cr2) {
        Some(owner) => fatal_msg(
            "#PF page fault",
            &f,
            Some(code.bits()),
            format_args!(
                "stack overflow in thread '{owner}': guard page hit at {cr2:#x} ({code:?})"
            ),
        ),
        None => fatal_msg(
            "#PF page fault",
            &f,
            Some(code.bits()),
            format_args!("page fault accessing {cr2:#x}: {code:?}"),
        ),
    }
}

extern "x86-interrupt" fn divide_error(f: InterruptStackFrame) {
    fatal("#DE divide error", &f, None)
}

extern "x86-interrupt" fn invalid_opcode(f: InterruptStackFrame) {
    fatal("#UD invalid opcode", &f, None)
}

extern "x86-interrupt" fn segment_not_present(f: InterruptStackFrame, code: u64) {
    fatal("#NP segment not present", &f, Some(code))
}

extern "x86-interrupt" fn stack_segment_fault(f: InterruptStackFrame, code: u64) {
    fatal("#SS stack segment fault", &f, Some(code))
}

// ---- remaining exceptions: report and halt ----

extern "x86-interrupt" fn debug_exception(f: InterruptStackFrame) {
    fatal_always("#DB debug exception", &f, None)
}

extern "x86-interrupt" fn nmi(f: InterruptStackFrame) {
    fatal_always("NMI non-maskable interrupt", &f, None)
}

extern "x86-interrupt" fn overflow(f: InterruptStackFrame) {
    fatal("#OF overflow", &f, None)
}

extern "x86-interrupt" fn bound_range(f: InterruptStackFrame) {
    fatal("#BR bound range exceeded", &f, None)
}

extern "x86-interrupt" fn device_not_available(f: InterruptStackFrame) {
    fatal("#NM device not available", &f, None)
}

extern "x86-interrupt" fn invalid_tss(f: InterruptStackFrame, code: u64) {
    fatal("#TS invalid TSS", &f, Some(code))
}

extern "x86-interrupt" fn x87_floating_point(f: InterruptStackFrame) {
    fatal("#MF x87 floating-point exception", &f, None)
}

extern "x86-interrupt" fn alignment_check(f: InterruptStackFrame, code: u64) {
    fatal("#AC alignment check", &f, Some(code))
}

extern "x86-interrupt" fn machine_check(f: InterruptStackFrame) -> ! {
    fatal_always("#MC machine check", &f, None)
}

extern "x86-interrupt" fn simd_floating_point(f: InterruptStackFrame) {
    fatal("#XM SIMD floating-point exception", &f, None)
}

extern "x86-interrupt" fn virtualization(f: InterruptStackFrame) {
    fatal_always("#VE virtualization exception", &f, None)
}

extern "x86-interrupt" fn cp_protection(f: InterruptStackFrame, code: u64) {
    fatal_always("#CP control protection", &f, Some(code))
}

extern "x86-interrupt" fn hv_injection(f: InterruptStackFrame) {
    fatal_always("#HV hypervisor injection exception", &f, None)
}

extern "x86-interrupt" fn vmm_communication(f: InterruptStackFrame, code: u64) {
    fatal_always("#VC VMM communication exception", &f, Some(code))
}

extern "x86-interrupt" fn security_exception(f: InterruptStackFrame, code: u64) {
    fatal_always("#SX security exception", &f, Some(code))
}

// ---- spurious PIC interrupts ----

/// Log the first few spurious IRQs (a flood would slow the ISR down on COM1).
fn note_spurious(slot: usize, irq: u8) {
    let n = SPURIOUS[slot].fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 4 {
        crate::klog!(Warn, "spurious IRQ{irq} ignored (count {n})");
    }
}

/// IRQ7. The master PIC raises it as a "spurious" vector when an interrupt
/// request vanished before acknowledgement; then bit 7 of the In-Service
/// Register is clear and the handler must return **without** an EOI (an EOI
/// would retire a real in-service IRQ of lower priority). If bit 7 is set it is
/// a genuine IRQ7 and needs the EOI.
extern "x86-interrupt" fn spurious_master(_f: InterruptStackFrame) {
    outb(PIC1_CMD, PIC_READ_ISR);
    let isr = inb(PIC1_CMD);
    outb(PIC1_CMD, PIC_READ_IRR);
    if isr & 0x80 != 0 {
        outb(PIC1_CMD, PIC_EOI);
    } else {
        note_spurious(0, 7);
    }
}

/// IRQ15 (slave PIC). A spurious one must not be acknowledged on the slave,
/// but the master did take the cascade interrupt (IRQ2), so it still gets an EOI.
extern "x86-interrupt" fn spurious_slave(_f: InterruptStackFrame) {
    outb(PIC2_CMD, PIC_READ_ISR);
    let isr = inb(PIC2_CMD);
    outb(PIC2_CMD, PIC_READ_IRR);
    if isr & 0x80 != 0 {
        outb(PIC2_CMD, PIC_EOI);
    } else {
        note_spurious(1, 15);
    }
    outb(PIC1_CMD, PIC_EOI);
}

// ---- 8259 PIC ----

/// Remap the PICs to 0x20/0x28 and unmask the timer, keyboard, and mouse.
fn remap_pic() {
    // ICW1: begin init (cascade, ICW4 needed).
    outb(PIC1_CMD, 0x11);
    outb(PIC2_CMD, 0x11);
    // ICW2: vector offsets.
    outb(PIC1_DATA, 0x20);
    outb(PIC2_DATA, 0x28);
    // ICW3: master/slave wiring (slave on IRQ2).
    outb(PIC1_DATA, 0x04);
    outb(PIC2_DATA, 0x02);
    // ICW4: 8086 mode.
    outb(PIC1_DATA, 0x01);
    outb(PIC2_DATA, 0x01);
    // Master: enable IRQ0 (timer), IRQ1 (keyboard), IRQ2 (cascade) -> 0xF8.
    outb(PIC1_DATA, 0xF8);
    // Slave: enable IRQ12 (mouse), bit 4 -> 0xEF.
    outb(PIC2_DATA, 0xEF);
}

// ---- 8253/8254 PIT ----

/// Run the PIT `SUB_PER_TICK` times faster (a video plays: `sched::present`) or at the tick rate.
/// The tick rate (`ticks()`) does not change; only the sub-ticks in between come and go.
pub fn set_fast(on: bool) {
    if FAST.load(Ordering::Relaxed) == on {
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        SUB.store(0, Ordering::Relaxed);
        FAST.store(on, Ordering::Relaxed);
        init_pit(if on {
            TIMER_HZ * SUB_PER_TICK
        } else {
            TIMER_HZ
        });
    });
}

/// Is the PIT running fast ([`set_fast`])?
pub fn fast() -> bool {
    FAST.load(Ordering::Relaxed)
}

fn init_pit(hz: u32) {
    let divisor = (1_193_182 / hz) as u16;
    outb(0x43, 0x36); // channel 0, lo/hi byte, mode 3 (square wave)
    outb(0x40, (divisor & 0xFF) as u8);
    outb(0x40, (divisor >> 8) as u8);
}
