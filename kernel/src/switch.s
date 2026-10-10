/*
 * Preemptive context switch — x86_64 (Intel syntax).
 *
 * Entered directly from the IDT on every PIT tick (IRQ0). The CPU has already
 * pushed the interrupt frame (SS, RSP, RFLAGS, CS, RIP) onto the *current*
 * thread's stack. We save the 15 general-purpose registers below it, hand the
 * resulting stack pointer to the Rust scheduler, switch RSP to whatever stack
 * it returns (the next thread), restore that thread's registers, and `iretq`.
 *
 * Because the entire interrupted state lives on each thread's own stack, the
 * `iretq` resumes a *different* thread exactly where it was preempted — no
 * cooperation required. This is the one place that cannot be written in Rust:
 * swapping the stack out from under the running code and returning elsewhere.
 *
 * The Rust half (sched::timer_schedule) saves the outgoing RSP, does round-robin
 * + FPU/SSE save/restore, EOIs the PIC, and returns the next thread's RSP in RAX.
 *
 * The same body is also installed on a software-interrupt vector (`yield_isr`,
 * `int 0x81`) so a thread that blocks can hand the CPU over immediately instead
 * of idling out the rest of its slice. Its Rust half (sched::yield_schedule)
 * neither advances the clock nor EOIs the PIC: it is not a hardware interrupt.
 */

.macro CONTEXT_SWITCH_ISR target
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp        /* arg0 = current (outgoing) stack pointer */
    call \target
    mov rsp, rax        /* switch to the next thread's stack        */
    RESTORE_AND_IRET
.endm

/* Pop a saved context (15 GPRs) off the current stack and `iretq` into it. */
.macro RESTORE_AND_IRET
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    iretq
.endm

.text
.global timer_isr
timer_isr:
    CONTEXT_SWITCH_ISR {schedule}

.global yield_isr
yield_isr:
    CONTEXT_SWITCH_ISR {yield_schedule}

/* `int 0x80` from ring 3 (gate DPL 3): the system-call entry. Same shape as the yield vector; the Rust
 * half decodes the saved registers, writes the result into the saved `rax` and returns the stack to
 * resume (its own, or another thread's when the call blocked or ended the process). */
.global syscall_isr
syscall_isr:
    CONTEXT_SWITCH_ISR {syscall_schedule}

/*
 * resume_context(rsp: u64) -> !   (System V: rsp in rdi)
 *
 * Jump into a thread whose context was saved by the ISR above, without
 * returning to the caller. Used to leave a thread that has just been killed:
 * the caller's own stack is simply abandoned.
 */
.global resume_context
resume_context:
    mov rsp, rdi
    RESTORE_AND_IRET
