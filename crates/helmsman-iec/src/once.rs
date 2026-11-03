//! A write-once cell that works without std: `std::sync::OnceLock` where
//! there is a std, and a lock-free leak-once cell on `no_std` (the first
//! initialiser to publish wins; a loser's value is dropped). Used for the
//! parsed standard library and a program's trace map.

#[cfg(feature = "std")]
pub struct Once<T>(std::sync::OnceLock<T>);

#[cfg(feature = "std")]
impl<T> Once<T> {
    pub const fn new() -> Self {
        Self(std::sync::OnceLock::new())
    }

    pub fn get_or_init(&self, f: impl FnOnce() -> T) -> &T {
        self.0.get_or_init(f)
    }

    pub fn get(&self) -> Option<&T> {
        self.0.get()
    }
}

#[cfg(feature = "std")]
impl<T: Clone> Clone for Once<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(not(feature = "std"))]
pub struct Once<T>(core::sync::atomic::AtomicPtr<T>);

#[cfg(not(feature = "std"))]
impl<T> Once<T> {
    pub const fn new() -> Self {
        Self(core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()))
    }

    pub fn get_or_init(&self, f: impl FnOnce() -> T) -> &T {
        use core::sync::atomic::Ordering;
        if let Some(v) = self.get() {
            return v;
        }
        let fresh = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(f()));
        match self
            .0
            .compare_exchange(core::ptr::null_mut(), fresh, Ordering::AcqRel, Ordering::Acquire)
        {
            // SAFETY: `fresh` came from Box::into_raw just above and is now
            // owned by the cell, which frees it only in Drop.
            Ok(_) => unsafe { &*fresh },
            Err(winner) => {
                // SAFETY: `fresh` was never shared; take it back and drop it.
                drop(unsafe { alloc::boxed::Box::from_raw(fresh) });
                // SAFETY: `winner` is a published Box pointer owned by the cell.
                unsafe { &*winner }
            }
        }
    }

    pub fn get(&self) -> Option<&T> {
        let p = self.0.load(core::sync::atomic::Ordering::Acquire);
        // SAFETY: a non-null pointer was published by get_or_init from a
        // Box the cell owns until it is dropped.
        (!p.is_null()).then(|| unsafe { &*p })
    }
}

#[cfg(not(feature = "std"))]
impl<T> Drop for Once<T> {
    fn drop(&mut self) {
        let p = self.0.load(core::sync::atomic::Ordering::Acquire);
        if !p.is_null() {
            // SAFETY: the one published Box, owned by this cell; `&mut self`
            // means no reader is alive.
            drop(unsafe { alloc::boxed::Box::from_raw(p) });
        }
    }
}

#[cfg(not(feature = "std"))]
impl<T: Clone> Clone for Once<T> {
    fn clone(&self) -> Self {
        let c = Self::new();
        if let Some(v) = self.get() {
            c.get_or_init(|| v.clone());
        }
        c
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for Once<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.get() {
            Some(v) => f.debug_tuple("Once").field(v).finish(),
            None => f.write_str("Once(<unset>)"),
        }
    }
}
