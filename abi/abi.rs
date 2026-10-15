//! Pebble ABI 1.0. This file has no runtime or Rust-library dependencies.
use core::ffi::c_void;
pub const ABI: u32 = 0x0001_0001;
pub const VOID: u32 = 0;
pub const U32: u32 = 1;
pub const F64: u32 = 2;
pub const UTF8: u32 = 3;
pub const BUFFER: u32 = 4;
pub const OBJECT: u32 = 5;
pub const HANDLE: u32 = 6;
/// Owned/borrowed value-tree bytes, specified in value.rs (ABI 1.1).
pub const TREE: u32 = 7;
pub const KIND_MASK: u32 = 0xffff;
/// Function signatures qualify OBJECT with a package-local type ID in bits
/// 16..31. Value.tag remains OBJECT; Value.reserved carries that type ID.
pub const TYPE_SHIFT: u32 = 16;
pub type Release = unsafe extern "C" fn(*mut c_void);
/// Input storage is borrowed until call returns. Output ownership transfers to
/// the host; release(owner) is called exactly once, including error paths.
/// OBJECT references list every managed handle retained by the native resource.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Value {
    pub tag: u32,
    pub reserved: u32,
    pub scalar: u64,
    pub data: *const u8,
    pub len: usize,
    pub owner: *mut c_void,
    pub release: Option<Release>,
    pub references: *const u64,
    pub references_len: usize,
}
impl Value {
    pub const fn scalar(tag: u32, scalar: u64) -> Self {
        Self {
            tag,
            reserved: 0,
            scalar,
            data: core::ptr::null(),
            len: 0,
            owner: core::ptr::null_mut(),
            release: None,
            references: core::ptr::null(),
            references_len: 0,
        }
    }
}
/// 0 success, 1 recoverable Exception, 2 fatal Error. Never unwind across C.
pub type Call = unsafe extern "C" fn(*const Value, usize, *mut Value) -> u32;
#[repr(C)]
pub struct Export {
    pub name: *const u8,
    pub name_len: usize,
    pub parameters: *const u32,
    pub parameters_len: usize,
    pub result: u32,
    pub reserved: u32,
    pub call: Call,
}
#[repr(C)]
pub struct Descriptor {
    pub abi: u32,
    pub size: u32,
    pub name: *const u8,
    pub name_len: usize,
    pub version: *const u8,
    pub version_len: usize,
    pub exports: *const Export,
    pub exports_len: usize,
}
// These tables are immutable static storage. Pointers refer to static arrays.
unsafe impl Sync for Export {}
unsafe impl Sync for Descriptor {}
pub type Entry = unsafe extern "C" fn() -> *const Descriptor;
