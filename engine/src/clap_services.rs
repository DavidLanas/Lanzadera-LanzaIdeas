// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 David Lanas
//! Servicios de main-thread para plugins CLAP (paso 1).
//! Timers, posix-fd, thread-check y despacho de on_main_thread.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use lanzadera_core::MAX_TRACKS;

// ── Identidad de hilos (RT-safe: thread-local, sin locks ni alloc) ──
thread_local! {
    static IS_MAIN: Cell<bool> = const { Cell::new(false) };
    static IS_AUDIO: Cell<bool> = const { Cell::new(false) };
}
pub fn mark_main_thread() { IS_MAIN.with(|c| c.set(true)); }
pub fn mark_audio_thread() { IS_AUDIO.with(|c| c.set(true)); }
pub fn is_main_thread() -> bool { IS_MAIN.with(|c| c.get()) }
pub fn is_audio_thread() -> bool { IS_AUDIO.with(|c| c.get()) }

// ── Wakeup de egui desde cualquier hilo (salvo el RT) ──
static WAKER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
pub fn set_waker(f: impl Fn() + Send + Sync + 'static) { let _ = WAKER.set(Box::new(f)); }
fn wake() {
    if is_audio_thread() { return; }
    if let Some(f) = WAKER.get() { f(); }
}

// ── Flags por pista ──
static CALLBACK: [AtomicBool; MAX_TRACKS] = [const { AtomicBool::new(false) }; MAX_TRACKS];
static RESTART:  [AtomicBool; MAX_TRACKS] = [const { AtomicBool::new(false) }; MAX_TRACKS];
static PROCESS:  [AtomicBool; MAX_TRACKS] = [const { AtomicBool::new(false) }; MAX_TRACKS];
static DIRTY:    [AtomicBool; MAX_TRACKS] = [const { AtomicBool::new(false) }; MAX_TRACKS];

pub fn request_callback(track: usize) {
    if track < MAX_TRACKS { CALLBACK[track].store(true, Ordering::Release); }
    wake();
}
pub fn request_restart(track: usize) {
    if track < MAX_TRACKS { RESTART[track].store(true, Ordering::Release); }
    wake();
}
pub fn request_process(track: usize) {
    if track < MAX_TRACKS { PROCESS[track].store(true, Ordering::Release); }
}
pub fn mark_dirty(track: usize) {
    if track < MAX_TRACKS { DIRTY[track].store(true, Ordering::Release); }
}
pub fn take_restart(track: usize) -> bool {
    track < MAX_TRACKS && RESTART[track].swap(false, Ordering::AcqRel)
}
pub fn take_dirty(track: usize) -> bool {
    track < MAX_TRACKS && DIRTY[track].swap(false, Ordering::AcqRel)
}
fn take_callback(track: usize) -> bool {
    track < MAX_TRACKS && CALLBACK[track].swap(false, Ordering::AcqRel)
}

// ── Timers ──
struct TimerEntry { track: usize, id: u32, period: Duration, next: Instant }
static TIMERS: Mutex<Vec<TimerEntry>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

pub fn register_timer(track: usize, period_ms: u32) -> u32 {
    let period = Duration::from_millis(period_ms.max(1) as u64);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut t) = TIMERS.lock() {
        t.push(TimerEntry { track, id, period, next: Instant::now() + period });
    }
    id
}
pub fn unregister_timer(track: usize, id: u32) {
    if let Ok(mut t) = TIMERS.lock() { t.retain(|e| !(e.track == track && e.id == id)); }
}
pub fn next_wakeup() -> Option<Duration> {
    let t = TIMERS.lock().ok()?;
    let now = Instant::now();
    let min = t.iter().map(|e| e.next.saturating_duration_since(now)).min()?;
    Some(min.max(Duration::from_millis(1)))
}

// ── Posix FD (solo Linux) ──
#[cfg(target_os = "linux")]
pub(crate) mod fds {
    use super::*;
    use clack_extensions::posix_fd::FdFlags;
    use std::os::unix::io::RawFd;

    pub struct FdEntry { pub track: usize, pub fd: RawFd, pub flags: FdFlags }
    pub static FDS: Mutex<Vec<FdEntry>> = Mutex::new(Vec::new());

    pub fn register(track: usize, fd: RawFd, flags: FdFlags) {
        if let Ok(mut v) = FDS.lock() {
            if let Some(e) = v.iter_mut().find(|e| e.track == track && e.fd == fd) {
                e.flags = flags;
            } else {
                v.push(FdEntry { track, fd, flags });
            }
        }
    }
    pub fn unregister(track: usize, fd: RawFd) {
        if let Ok(mut v) = FDS.lock() { v.retain(|e| !(e.track == track && e.fd == fd)); }
    }
    pub fn still_registered(track: usize, fd: RawFd) -> bool {
        FDS.lock().map(|v| v.iter().any(|e| e.track == track && e.fd == fd)).unwrap_or(false)
    }
    pub fn poll(instances: &mut [(usize, crate::clap_host::Instance)]) {
        use clack_extensions::posix_fd::PluginPosixFd;
        let snap: Vec<FdEntry> = match FDS.lock() {
            Ok(v) => v.iter().map(|e| FdEntry { track: e.track, fd: e.fd, flags: e.flags }).collect(),
            Err(_) => return,
        };
        if snap.is_empty() { return; }
        let mut pfds: Vec<libc::pollfd> = snap.iter().map(|e| {
            let mut ev = 0i16;
            if e.flags.contains(FdFlags::READ)  { ev |= libc::POLLIN; }
            if e.flags.contains(FdFlags::WRITE) { ev |= libc::POLLOUT; }
            if e.flags.contains(FdFlags::ERROR) { ev |= libc::POLLERR; }
            libc::pollfd { fd: e.fd, events: ev, revents: 0 }
        }).collect();
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, 0) };
        if n <= 0 { return; }
        for (e, p) in snap.iter().zip(pfds.iter()) {
            if p.revents == 0 { continue; }
            let mut ready = FdFlags::empty();
            if p.revents & libc::POLLIN  != 0 { ready |= FdFlags::READ; }
            if p.revents & libc::POLLOUT != 0 { ready |= FdFlags::WRITE; }
            if p.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 { ready |= FdFlags::ERROR; }
            let ready = ready & e.flags;
            if ready.is_empty() || !still_registered(e.track, e.fd) { continue; }
            if let Some((_, inst)) = instances.iter_mut().find(|(t, _)| *t == e.track) {
                let main = inst.plugin_handle();
                if let Some(ext) = main.get_extension::<PluginPosixFd>() {
                    ext.on_fd(&main, e.fd, ready);
                }
            }
        }
    }
}

pub fn clear_track(track: usize) {
    if let Ok(mut t) = TIMERS.lock() { t.retain(|e| e.track != track); }
    #[cfg(target_os = "linux")]
    if let Ok(mut v) = fds::FDS.lock() { v.retain(|e| e.track != track); }
    if track < MAX_TRACKS {
        CALLBACK[track].store(false, Ordering::Relaxed);
        RESTART[track].store(false, Ordering::Relaxed);
        PROCESS[track].store(false, Ordering::Relaxed);
        DIRTY[track].store(false, Ordering::Relaxed);
    }
}

/// Despacho por frame (hilo principal): callbacks + timers + fds.
pub fn poll(instances: &mut [(usize, crate::clap_host::Instance)]) {
    use clack_extensions::timer::{PluginTimer, TimerId};

    for (track, inst) in instances.iter_mut() {
        if take_callback(*track) {
            inst.call_on_main_thread_callback();
        }
    }

    let due: Vec<(usize, u32)> = {
        let now = Instant::now();
        let mut out = Vec::new();
        if let Ok(mut t) = TIMERS.lock() {
            for e in t.iter_mut() {
                if e.next <= now { e.next = now + e.period; out.push((e.track, e.id)); }
            }
        }
        out
    };
    for (track, id) in due {
        let alive = TIMERS.lock().map(|t| t.iter().any(|e| e.track == track && e.id == id)).unwrap_or(false);
        if !alive { continue; }
        if let Some((_, inst)) = instances.iter_mut().find(|(t, _)| *t == track) {
            let main = inst.plugin_handle();
            if let Some(ext) = main.get_extension::<PluginTimer>() {
                ext.on_timer(&main, TimerId(id));
            }
        }
    }

    #[cfg(target_os = "linux")]
    fds::poll(instances);
}
