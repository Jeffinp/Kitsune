//! The fetcher thread: waits for a request, serves it and keeps the network stack alive between fetches.

use super::*;

/// Worker thread entry. Processes one queued request at a time and, while idle,
/// is *blocked* in the scheduler (it costs no time slice at all) until
/// [`try_post`] wakes it.
pub extern "C" fn worker() -> ! {
    TID.store(crate::sched::current(), Ordering::Release);
    // Confirm the time over SNTP before anything else: certificate validity dates
    // are only as good as the clock. A request posted meanwhile waits (at most a
    // few seconds); if no server answers the RTC is used and the browser says
    // "hora nao confirmada" when a date check fails.
    // SAFETY: NET is set once before this thread exists and used only by this worker.
    if let Some(netd) = unsafe { (*NET.get()).as_mut() } {
        netd.net_mut().sync_time();
        LAST_RESYNC_MS.store(crate::netd::now_ms().max(1), Ordering::Relaxed);
    }
    loop {
        if STATE.load(Ordering::Acquire) == REQUESTED {
            STATE.store(RUNNING, Ordering::Relaxed);
            // SAFETY: STATE == REQUESTED (Acquire) pairs with `try_post`'s Release, so REQ_URL/REQ_LEN are
            // complete; the compositor will not rewrite them until DONE -> IDLE.
            let url = unsafe {
                let n = *REQ_LEN.get();
                let buf = &*REQ_URL.get();
                buf[..n].to_vec()
            };
            // SAFETY: as for REQ_URL above (REQ_INSECURE* are written together with it).
            let insecure = unsafe {
                let n = *REQ_INSECURE_LEN.get();
                (&*REQ_INSECURE.get())[..n].to_vec()
            };
            // SAFETY: as for REQ_URL above (written with it, before REQUESTED).
            let body = unsafe { (*REQ_BODY.get()).take() };
            let kind = REQ_KIND.load(Ordering::Relaxed);
            let fit_w = REQ_FIT_W.load(Ordering::Relaxed);
            let app = WHO.load(Ordering::Acquire) != WHO_BROWSER;
            let policy = if app {
                // SAFETY: written by the app's poster before the Release store of REQUESTED that this
                // worker saw (Acquire) above; not touched again until the slot is back to IDLE.
                unsafe { (*APP_POLICY.get()).clone() }
            } else {
                None
            };
            // SAFETY: NET is set once, before this thread exists (`fetch::init`), and used only by this
            // worker, one request at a time, so the `&mut` is unique.
            let netd = unsafe { (*NET.get()).as_mut() };
            if kind == KIND_SHEET || kind == KIND_SCRIPT {
                let result = match netd {
                    Some(netd) => fetch_sheet(netd.net_mut(), &url, &insecure, kind),
                    None => None,
                };
                // SAFETY: STATE is RUNNING; the compositor reads SHEET_RESULT only after seeing DONE
                // (Release store right after this write).
                unsafe {
                    *SHEET_RESULT.get() = Some(result);
                }
                STATE.store(DONE, Ordering::Release);
            } else if kind == KIND_IMAGE {
                let result = match netd {
                    Some(netd) => fetch_image(netd.net_mut(), &url, &insecure, fit_w),
                    None => Err(ImgFail::Failed),
                };
                // SAFETY: STATE is RUNNING; the compositor reads IMG_RESULT only after seeing DONE
                // (Release store right after this write).
                unsafe {
                    *IMG_RESULT.get() = Some(result);
                }
                STATE.store(DONE, Ordering::Release);
            } else {
                let result = match netd {
                    Some(netd) => {
                        let net = netd.net_mut();
                        // An app only reaches public addresses, checked on what the name resolved to.
                        net.set_public_only(app);
                        let r = match (app, policy.as_ref()) {
                            (false, _) => fetch_url(
                                net,
                                &url,
                                &insecure,
                                MAX_RESPONSE_BYTES,
                                None,
                                body.as_deref(),
                                cookies::Use::Navigation,
                            ),
                            (true, Some(p)) => fetch_url(
                                net,
                                &url,
                                &[],
                                MAX_RESPONSE_BYTES,
                                Some(p),
                                None,
                                cookies::Use::None,
                            ),
                            // An app request without a policy cannot happen; refuse rather than guess.
                            (true, None) => Err(FailReason::Network),
                        };
                        net.set_public_only(false);
                        r
                    }
                    None => Err(FailReason::Network),
                };
                if WHO.load(Ordering::Acquire) == WHO_ABANDONED {
                    // The app timed out and left: nobody wants this result.
                    drop(result);
                    WHO.store(WHO_BROWSER, Ordering::Release);
                    STATE.store(IDLE, Ordering::Release);
                } else {
                    // SAFETY: STATE is RUNNING here; the poster only touches RESULT after seeing DONE,
                    // which is stored (Release) right after this write.
                    unsafe {
                        *RESULT.get() = Some(result);
                    }
                    STATE.store(DONE, Ordering::Release);
                    if app {
                        let tid = APP_TID.load(Ordering::Acquire);
                        if tid != usize::MAX {
                            crate::sched::wake(tid);
                        }
                    }
                }
            }
        } else if job_pending() {
            // SAFETY: NET is used only by this worker thread.
            if let Some(netd) = unsafe { (*NET.get()).as_mut() } {
                serve_job(netd.net_mut());
            }
        } else if script_pending() {
            // A page script's request (`fetch`, XHR): after the browser's own and the shell's.
            // SAFETY: NET is used only by this worker thread.
            match unsafe { (*NET.get()).as_mut() } {
                Some(netd) => serve_script(netd.net_mut()),
                None => drop_scripts(),
            }
        } else {
            // Idle: service the network (DHCP timers, ARP/ping responder, ping client),
            // then leave the run queue until a request is posted (`try_post` and
            // `ping_start` wake us) or the service interval ends. The scheduler
            // re-checks the condition after announcing the block, so a request posted
            // in between is not missed.
            // Fold the entropy sample ring into the pool and reseed when due (cheap when idle).
            crate::rng::service();
            // SAFETY: as above: NET is used only by this worker thread.
            let sleep = match unsafe { (*NET.get()).as_mut() } {
                Some(netd) => {
                    // Kept-alive connections idle for too long are closed here too.
                    netd.net_mut().expire_idle();
                    netd.service()
                }
                None => crate::netd::IDLE_POLL_TICKS,
            };
            crate::sched::block(crate::interrupts::ticks() + sleep, || {
                STATE.load(Ordering::Acquire) != REQUESTED
                    && !crate::netd::ping_pending()
                    && !job_pending()
                    && !script_pending()
            });
        }
    }
}

/// Monotonic ms of the last SNTP attempt made on behalf of a page load.
pub(super) static LAST_RESYNC_MS: AtomicU64 = AtomicU64::new(0);

/// Retry the SNTP sync before an HTTPS load when the clock is still
/// unconfirmed, rate-limited to one attempt per minute.
pub(super) fn resync_clock_if_needed(net: &mut netstack::Net) {
    if crate::clock::confirmed() {
        return;
    }
    let now = crate::netd::now_ms();
    let last = LAST_RESYNC_MS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < 60_000 {
        return;
    }
    LAST_RESYNC_MS.store(now.max(1), Ordering::Relaxed);
    net.sync_time();
}
