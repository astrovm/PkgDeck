//! Handing memory back to the system. The allocator keeps what PkgDeck
//! frees for later, which in the tray means holding on to a window's or a
//! check's worth of memory for nothing.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
mod native {
    use std::ffi::c_int;
    unsafe extern "C" {
        fn mallopt(param: c_int, value: c_int) -> c_int;
        fn malloc_trim(pad: usize) -> c_int;
    }
    const M_MMAP_THRESHOLD: c_int = -3;
    const M_ARENA_MAX: c_int = -8;

    pub fn tune() {
        // SAFETY: plain glibc calls, made before other threads allocate.
        unsafe {
            // Each thread that allocates may otherwise get its own arena,
            // and every arena keeps what was freed in it.
            mallopt(M_ARENA_MAX, 2);
            // Large buffers, such as a block read out of an AppImage, go
            // straight back when freed instead of staying in the heap.
            mallopt(M_MMAP_THRESHOLD, 1024 * 1024);
        }
    }
    pub fn release() {
        // SAFETY: glibc's own call; it locks each arena while trimming.
        unsafe {
            malloc_trim(0);
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod native {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    }
    pub fn tune() {}
    pub fn release() {
        // SAFETY: a null zone asks every zone; a zero goal frees all it can.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}

#[cfg(not(any(all(target_os = "linux", target_env = "gnu"), target_os = "macos")))]
mod native {
    pub fn tune() {}
    pub fn release() {}
}

/// Sets the allocator up to give memory back. Call first thing in `main`.
pub fn tune() {
    native::tune();
}

/// Gives what was freed back to the system.
pub fn release() {
    native::release();
}
