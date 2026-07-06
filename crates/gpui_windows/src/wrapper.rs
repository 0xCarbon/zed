use std::{num::NonZeroIsize, ops::Deref};

use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::HCURSOR};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SafeCursor {
    raw: HCURSOR,
}

unsafe impl Send for SafeCursor {}
unsafe impl Sync for SafeCursor {}

impl From<HCURSOR> for SafeCursor {
    fn from(value: HCURSOR) -> Self {
        SafeCursor { raw: value }
    }
}

impl Deref for SafeCursor {
    type Target = HCURSOR;

    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SafeHwnd {
    raw: HWND,
}

impl SafeHwnd {
    pub(crate) fn as_raw(&self) -> HWND {
        self.raw
    }
}

unsafe impl Send for SafeHwnd {}
unsafe impl Sync for SafeHwnd {}

impl From<HWND> for SafeHwnd {
    fn from(value: HWND) -> Self {
        SafeHwnd { raw: value }
    }
}

impl Deref for SafeHwnd {
    type Target = HWND;

    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}

/// A window handle that is guaranteed to be non-null. Because it wraps a
/// `NonZeroIsize`, it is automatically `Send + Sync` with no `unsafe impl`
/// needed (unlike `SafeHwnd`, which opts in explicitly). The HWND it carries is
/// a process-wide handle that is safe to move and share between threads, which
/// is the same rationale `SafeHwnd` documents for its explicit opt-in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NonNullHwnd(NonZeroIsize);

impl NonNullHwnd {
    pub(crate) fn new(raw: HWND) -> Option<Self> {
        NonZeroIsize::new(raw.0 as isize).map(Self)
    }

    pub(crate) fn hwnd(self) -> HWND {
        HWND(self.0.get() as _)
    }

    pub(crate) fn as_non_zero_isize(self) -> NonZeroIsize {
        self.0
    }
}
