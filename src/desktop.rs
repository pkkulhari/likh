use crate::config::PasteShortcut;
use anyhow::{Result, bail, ensure};
use libc::{c_char, c_int, c_ulong, c_void};
use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    rc::Rc,
};

#[repr(C)]
#[derive(Default)]
pub struct Event {
    pub kind: c_int,
    pub window: c_ulong,
}
unsafe extern "C" {
    fn lx_open(hotkey: c_int, error: *mut c_char, cap: usize) -> *mut c_void;
    fn lx_close(x: *mut c_void);
    fn lx_indicator(x: *mut c_void, label: *const c_char);
    fn lx_activity(x: *mut c_void, mode: c_int);
    fn lx_level(x: *mut c_void, level: f32);
    fn lx_busy(x: *mut c_void, busy: c_int);
    fn lx_focus(x: *mut c_void) -> c_ulong;
    fn lx_track(x: *mut c_void, w: c_ulong);
    fn lx_focus_valid(x: *mut c_void) -> c_int;
    fn lx_modifiers_down(x: *mut c_void) -> c_int;
    fn lx_class(x: *mut c_void, w: c_ulong, buf: *mut c_char, cap: usize);
    fn lx_snapshot(x: *mut c_void) -> c_int;
    fn lx_snapshot_current(x: *mut c_void) -> c_int;
    fn lx_set_clipboard(x: *mut c_void, data: *const u8, len: usize) -> c_int;
    fn lx_restore(x: *mut c_void) -> c_int;
    fn lx_target_requestor(x: *mut c_void, w: c_ulong) -> c_int;
    fn lx_paste(x: *mut c_void, shortcut: c_int) -> c_int;
    fn lx_next(x: *mut c_void, timeout: c_int, event: *mut Event) -> c_int;
}
// !Send and !Sync: Xlib handles and all native calls stay on their owning thread.
pub struct Desktop {
    ptr: *mut c_void,
    _thread: PhantomData<Rc<()>>,
}
#[repr(i32)]
pub enum Activity {
    Starting = 2,
    Listening = 3,
    Processing = 4,
}
impl Desktop {
    pub fn open(hotkey: bool) -> Result<Self> {
        ensure!(
            std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland"),
            "Wayland is not supported yet. Log into an Xorg/X11 desktop session."
        );
        let mut error: [c_char; 256] = [0; 256];
        let ptr = unsafe { lx_open(hotkey as i32, error.as_mut_ptr(), error.len()) };
        if ptr.is_null() {
            bail!(
                "{}",
                unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
            );
        }
        Ok(Self {
            ptr,
            _thread: PhantomData,
        })
    }
    pub fn indicator(&self, label: &str) {
        if let Ok(label) = CString::new(label) {
            unsafe { lx_indicator(self.ptr, label.as_ptr()) };
        }
    }
    pub fn activity(&self, activity: Activity) {
        unsafe { lx_activity(self.ptr, activity as i32) }
    }
    pub fn level(&self, level: f32) {
        unsafe { lx_level(self.ptr, level) }
    }
    pub fn busy(&self, busy: bool) {
        unsafe { lx_busy(self.ptr, busy as i32) }
    }
    pub fn track(&self) -> String {
        let window = unsafe { lx_focus(self.ptr) };
        unsafe { lx_track(self.ptr, window) };
        let mut class: [c_char; 256] = [0; 256];
        unsafe { lx_class(self.ptr, window, class.as_mut_ptr(), class.len()) };
        unsafe { CStr::from_ptr(class.as_ptr()) }
            .to_string_lossy()
            .to_string()
    }
    pub fn valid_focus(&self) -> bool {
        unsafe { lx_focus_valid(self.ptr) != 0 }
    }
    pub fn modifiers_down(&self) -> bool {
        unsafe { lx_modifiers_down(self.ptr) != 0 }
    }
    pub fn snapshot(&self) -> Result<bool> {
        let n = unsafe { lx_snapshot(self.ptr) };
        ensure!(n >= 0, "Clipboard snapshot failed");
        Ok(n == 1)
    }
    pub fn set_clipboard(&self, text: &str) -> Result<()> {
        ensure!(
            unsafe { lx_set_clipboard(self.ptr, text.as_ptr(), text.len()) } != 0,
            "Cannot take clipboard ownership"
        );
        Ok(())
    }
    pub fn snapshot_current(&self) -> bool {
        unsafe { lx_snapshot_current(self.ptr) != 0 }
    }
    pub fn restore(&self) {
        unsafe { lx_restore(self.ptr) };
    }
    pub fn target_requestor(&self, window: u64) -> bool {
        unsafe { lx_target_requestor(self.ptr, window) != 0 }
    }
    pub fn paste(&self, shortcut: PasteShortcut) -> Result<()> {
        ensure!(
            unsafe {
                lx_paste(
                    self.ptr,
                    match shortcut {
                        PasteShortcut::CtrlV => 0,
                        PasteShortcut::CtrlShiftV => 1,
                        PasteShortcut::ShiftInsert => 2,
                    },
                )
            } != 0,
            "Focus, keyboard, or clipboard changed before paste; use `likh copy-last`"
        );
        Ok(())
    }
    pub fn next(&self, timeout: i32) -> Option<Event> {
        let mut event = Event::default();
        (unsafe { lx_next(self.ptr, timeout, &mut event) } != 0).then_some(event)
    }
}
impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe { lx_close(self.ptr) }
    }
}
