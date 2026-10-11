//! Ring 3 host for a WebAssembly app: `wasmi` runs here, every `osj.*` import goes to the kernel.
//! The wire contract is `kitsune_core::appabi::remote`.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::{self, NonNull};

use kitsune_core::appabi::remote::*;
use kitsune_user::{HEAP_BASE, brk, entry, exit, syscall3};
use linked_list_allocator::Heap;
use wasmi::errors::HostError;
use wasmi::{
    Caller, Config, Engine, Extern, ExternType, Func, FuncType, Instance, Linker, Module, Store,
    StoreLimits, StoreLimitsBuilder, TrapCode, Val, ValType,
};

// ---- heap ----

const MIB: u64 = 1 << 20;

struct HeapState {
    heap: Heap,
    /// Current end of the heap as granted by `brk` (0 = not initialised).
    end: u64,
}

struct Allocator(UnsafeCell<HeapState>);

// SAFETY: the process has a single thread.
unsafe impl Sync for Allocator {}

#[global_allocator]
static ALLOC: Allocator = Allocator(UnsafeCell::new(HeapState {
    heap: Heap::empty(),
    end: 0,
}));

impl HeapState {
    fn init(&mut self) {
        let end = brk(HEAP_BASE + MIB);
        if end >= HEAP_BASE + MIB {
            // SAFETY: [HEAP_BASE, end) was just granted by the kernel and nothing else uses it.
            unsafe {
                self.heap
                    .init(HEAP_BASE as *mut u8, (end - HEAP_BASE) as usize)
            };
            self.end = end;
        } else {
            self.end = u64::MAX; // no heap at all: every allocation fails
        }
    }

    fn grow(&mut self, need: usize) -> bool {
        if self.end == u64::MAX {
            return false;
        }
        let want = (need as u64).saturating_add(MIB - 1) / MIB * MIB;
        let target = self.end.saturating_add(want.max(MIB));
        let got = brk(target);
        if got <= self.end {
            return false;
        }
        let by = (got - self.end) as usize;
        self.end = got;
        // SAFETY: the new range directly follows the heap and is now mapped.
        unsafe { self.heap.extend(by) };
        true
    }
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: single thread, no reentrancy (brk does not allocate).
        let st = unsafe { &mut *self.0.get() };
        if st.end == 0 {
            st.init();
        }
        for _ in 0..2 {
            if let Ok(p) = st.heap.allocate_first_fit(layout) {
                return p.as_ptr();
            }
            // Room for the block plus the allocator's bookkeeping and alignment slack.
            if !st.grow(
                layout
                    .size()
                    .saturating_add(layout.align())
                    .saturating_add(64),
            ) {
                break;
            }
        }
        match st.heap.allocate_first_fit(layout) {
            Ok(p) => p.as_ptr(),
            Err(()) => ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: single thread.
        let st = unsafe { &mut *self.0.get() };
        if let Some(p) = NonNull::new(p) {
            // SAFETY: `p` came from `alloc` with this layout.
            unsafe { st.heap.deallocate(p, layout) };
        }
    }
}

// ---- guest state ----

struct Data {
    limits: StoreLimits,
}

/// The kernel refused a host call as a fault of the guest: trap.
#[derive(Debug)]
struct GuestFault;

impl core::fmt::Display for GuestFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("host call refused")
    }
}

impl HostError for GuestFault {}

struct Guest {
    store: Store<Data>,
    instance: Instance,
}

fn ty_of(t: Ty) -> ValType {
    match t {
        Ty::I32 => ValType::I32,
        Ty::I64 => ValType::I64,
    }
}

fn sig_matches(ty: &FuncType, f: &HostFn) -> bool {
    let same = |have: &[ValType], want: &[Ty]| {
        have.len() == want.len() && have.iter().zip(want).all(|(h, w)| *h == ty_of(*w))
    };
    same(ty.params(), f.params) && same(ty.results(), f.results)
}

fn host_call(
    id: u32,
    caller: &mut Caller<'_, Data>,
    params: &[Val],
    results: &mut [Val],
) -> Result<(), wasmi::Error> {
    let f = &HOST_FNS[id as usize];
    let mut rec = HostCall {
        id,
        nargs: params.len().min(6) as u32,
        ..HostCall::default()
    };
    for (slot, v) in rec.args.iter_mut().zip(params) {
        *slot = match v {
            Val::I32(x) => *x as i64 as u64,
            Val::I64(x) => *x as u64,
            _ => 0,
        };
    }
    let mem = ["memory", "mem"]
        .iter()
        .find_map(|n| caller.get_export(n).and_then(Extern::into_memory));
    if let Some(m) = mem {
        rec.mem_base = m.data_ptr(&*caller) as u64;
        rec.mem_len = m.data_size(&*caller) as u64;
    }
    let bytes = rec.to_bytes();
    // SAFETY: `bytes` is a valid 72-byte record for the duration of the call.
    let r = unsafe { syscall3(SYS_HOST_CALL, bytes.as_ptr() as u64, 0, 0) };
    if r == CALL_FAULT {
        return Err(wasmi::Error::host(GuestFault));
    }
    if r == CALL_EXIT {
        return Err(wasmi::Error::i32_exit(rec.args[0] as i32));
    }
    if let (Some(slot), Some(t)) = (results.first_mut(), f.results.first()) {
        *slot = match t {
            Ty::I32 => Val::I32(r as i32),
            Ty::I64 => Val::I64(r),
        };
    }
    Ok(())
}

/// Truncate `s` to at most MAX_MSG bytes on a char boundary.
fn fit(s: &str) -> &str {
    let mut n = s.len().min(MAX_MSG);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

fn pages(g: &Option<Guest>) -> u64 {
    let Some(g) = g else { return 0 };
    ["memory", "mem"]
        .iter()
        .find_map(|n| {
            g.instance
                .get_export(&g.store, n)
                .and_then(Extern::into_memory)
        })
        .map_or(0, |m| m.size(&g.store))
}

fn send_done(status: u32, code: i32, fuel_spent: u64, mem_pages: u64, msg: &str) {
    let msg = fit(msg);
    let done = Done {
        status,
        code,
        fuel_spent,
        mem_pages,
        msg_ptr: if msg.is_empty() {
            0
        } else {
            msg.as_ptr() as u64
        },
        msg_len: msg.len() as u64,
    };
    let bytes = done.to_bytes();
    // SAFETY: the record and `msg` live until the call returns.
    unsafe { syscall3(SYS_HOST_JOB_DONE, bytes.as_ptr() as u64, 0, 0) };
}

fn init(module_bytes: &[u8], job: &Job) -> Result<Guest, String> {
    let mut cfg = Config::default();
    cfg.consume_fuel(true);
    let engine = Engine::new(&cfg);
    let module = Module::new(&engine, module_bytes).map_err(|e| format!("{e}"))?;
    let mem_bytes = usize::try_from(job.mem_bytes).unwrap_or(usize::MAX);
    let mut store = Store::new(
        &engine,
        Data {
            limits: StoreLimitsBuilder::new()
                .memory_size(mem_bytes)
                .table_elements(100_000)
                .instances(1)
                .memories(1)
                .tables(1)
                .build(),
        },
    );
    store.limiter(|d| &mut d.limits);
    store.set_fuel(job.fuel).map_err(|e| format!("{e}"))?;
    let mut linker = <Linker<Data>>::new(&engine);
    for imp in module.imports() {
        let ExternType::Func(fty) = imp.ty() else {
            if imp.module() == HOST_MODULE {
                return Err(format!("unsupported import osj.{}", imp.name()));
            }
            return Err(format!("unknown import {}.{}", imp.module(), imp.name()));
        };
        if imp.module() != HOST_MODULE {
            return Err(format!("unknown import {}.{}", imp.module(), imp.name()));
        }
        let Some(id) = host_fn_id(imp.name()) else {
            return Err(format!("unknown import osj.{}", imp.name()));
        };
        if !sig_matches(fty, &HOST_FNS[id as usize]) {
            return Err(format!("wrong signature for import osj.{}", imp.name()));
        }
        // Several imports of the same name are legal in wasm; the linker rejects a second
        // definition, so skip it.
        let _ = linker.func_new(
            HOST_MODULE,
            imp.name(),
            fty.clone(),
            move |mut caller, params, results| host_call(id, &mut caller, params, results),
        );
    }
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| format!("{e}"))?;
    Ok(Guest { store, instance })
}

fn read_module() -> Vec<u8> {
    // SAFETY: no arguments.
    let n = unsafe { syscall3(SYS_HOST_MODULE_LEN, 0, 0, 0) };
    if n < 0 {
        exit(2);
    }
    let len = n as usize;
    let mut buf: Vec<u8> = Vec::new();
    if buf.try_reserve_exact(len).is_err() {
        exit(2);
    }
    buf.resize(len, 0);
    let mut got = 0usize;
    while got < len {
        // SAFETY: the pointer/cap describe the unfilled tail of `buf`.
        let r = unsafe {
            syscall3(
                SYS_HOST_MODULE_READ,
                buf.as_mut_ptr().add(got) as u64,
                (len - got) as u64,
                0,
            )
        };
        if r <= 0 {
            exit(2);
        }
        got += r as usize;
    }
    buf
}

fn do_call(guest: &mut Option<Guest>, job: &Job) {
    let Some(g) = guest.as_mut() else {
        send_done(DONE_LOAD_FAILED, 0, 0, 0, "no instance");
        return;
    };
    let Some(name) = EXPORT_NAMES.get(job.export as usize) else {
        send_done(DONE_OK, 0, 0, pages(guest), "");
        return;
    };
    let Some(f) = g.instance.get_func(&g.store, name) else {
        send_done(DONE_OK, 0, 0, pages(guest), "");
        return;
    };
    let fty = f.ty(&g.store);
    let params = fty.params();
    if params.len() > 4 || params.iter().any(|p| *p != ValType::I32) || !fty.results().is_empty() {
        send_done(DONE_OK, 0, 0, pages(guest), ""); // incompatible signature: ignore the hook
        return;
    }
    let vals = [
        Val::I32(job.args[0]),
        Val::I32(job.args[1]),
        Val::I32(job.args[2]),
        Val::I32(job.args[3]),
    ];
    if g.store.set_fuel(job.fuel).is_err() {
        send_done(DONE_TRAP, 0, 0, pages(guest), "cannot set fuel");
        return;
    }
    let r = Func::call(&f, &mut g.store, &vals[..params.len()], &mut []);
    let spent = job.fuel.saturating_sub(g.store.get_fuel().unwrap_or(0));
    let mp = pages(guest);
    match r {
        Ok(()) => send_done(DONE_OK, 0, spent, mp, ""),
        Err(e) => {
            if e.downcast_ref::<GuestFault>().is_some() {
                send_done(DONE_FAULT, 0, spent, mp, "");
            } else if let Some(code) = e.i32_exit_status() {
                send_done(DONE_EXIT, code, spent, mp, "");
            } else if e.as_trap_code() == Some(TrapCode::OutOfFuel) {
                send_done(DONE_OUT_OF_FUEL, 0, spent, mp, "");
            } else {
                let m = format!("{e}");
                send_done(DONE_TRAP, 0, spent, mp, &m);
            }
        }
    }
}

entry!(main);

fn main() -> i32 {
    let module = read_module();
    let mut guest: Option<Guest> = None;
    loop {
        let mut raw = [0u8; 40];
        // SAFETY: `raw` is a valid 40-byte buffer.
        let r = unsafe { syscall3(SYS_HOST_JOB_WAIT, raw.as_mut_ptr() as u64, 0, 0) };
        if r < 0 {
            return 3;
        }
        let job = Job::from_bytes(&raw);
        match job.kind {
            JOB_INIT => {
                guest = None;
                match init(&module, &job) {
                    Ok(g) => {
                        let spent = job.fuel.saturating_sub(g.store.get_fuel().unwrap_or(0));
                        guest = Some(g);
                        send_done(DONE_OK, 0, spent, pages(&guest), "");
                    }
                    Err(msg) => send_done(DONE_LOAD_FAILED, 0, 0, 0, &msg),
                }
            }
            JOB_CALL => do_call(&mut guest, &job),
            JOB_QUIT => return 0,
            _ => send_done(DONE_LOAD_FAILED, 0, 0, 0, "unknown job"),
        }
    }
}
