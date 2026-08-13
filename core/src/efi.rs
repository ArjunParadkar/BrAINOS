//! Minimal hand-rolled UEFI FFI — the only thing between the portable core
//! and bare firmware. No external crates. Everything above this file is
//! architecture-blind (BrAInOS Architecture §16.2).

#![allow(dead_code)]

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Event = *mut c_void;
pub type Status = usize;

pub const SUCCESS: Status = 0;
pub const NOT_READY: Status = 6 | (1 << (usize::BITS - 1));

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

pub const GOP_GUID: Guid = Guid(
    0x9042a9de, 0x23dc, 0x4a38,
    [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
);
pub const LOADED_IMAGE_GUID: Guid = Guid(
    0x5b1b31a1, 0x9562, 0x11d2,
    [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
);
pub const SIMPLE_FS_GUID: Guid = Guid(
    0x964e5b22, 0x6459, 0x11d2,
    [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
);
/// EFI_SIMPLE_NETWORK_PROTOCOL — a NIC the firmware can drive raw.
pub const SNP_GUID: Guid = Guid(
    0xa19832b9, 0xac25, 0x11d3,
    [0x9a, 0x2d, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d],
);
/// EFI_HTTP_SERVICE_BINDING_PROTOCOL — firmware-provided HTTP client
/// (present when the platform was built with the UEFI network stack).
pub const HTTP_SB_GUID: Guid = Guid(
    0xbdc8e6af, 0xd9bc, 0x4379,
    [0xa7, 0x2a, 0xe0, 0xc4, 0xe7, 0x5d, 0xae, 0x1c],
);
/// EFI_HTTP_PROTOCOL — the child protocol the service binding mints.
pub const HTTP_GUID: Guid = Guid(
    0x7a59b29b, 0x910b, 0x4171,
    [0x82, 0x42, 0xa8, 0x5a, 0x0d, 0xf2, 0x5b, 0x5b],
);
/// EFI_IP4_CONFIG2_PROTOCOL — per-NIC address configuration (DHCP policy).
pub const IP4_CONFIG2_GUID: Guid = Guid(
    0x5b446ed1, 0xe30b, 0x4faa,
    [0x87, 0x1a, 0x36, 0x54, 0xec, 0xa3, 0x60, 0x80],
);

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *const u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut SimpleTextInput,
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutput,
    pub stderr_handle: Handle,
    pub std_err: *mut SimpleTextOutput,
    pub runtime_services: *mut RuntimeServices,
    pub boot_services: *mut BootServices,
    pub number_of_table_entries: usize,
    pub configuration_table: *mut c_void,
}

#[repr(C)]
pub struct SimpleTextOutputMode {
    pub max_mode: i32,
    pub mode: i32,
    pub attribute: i32,
    pub cursor_column: i32,
    pub cursor_row: i32,
    pub cursor_visible: u8,
}

#[repr(C)]
pub struct SimpleTextOutput {
    pub reset: extern "efiapi" fn(*mut Self, u8) -> Status,
    pub output_string: extern "efiapi" fn(*mut Self, *const u16) -> Status,
    pub test_string: extern "efiapi" fn(*mut Self, *const u16) -> Status,
    pub query_mode:
        extern "efiapi" fn(*mut Self, usize, *mut usize, *mut usize) -> Status,
    pub set_mode: extern "efiapi" fn(*mut Self, usize) -> Status,
    pub set_attribute: extern "efiapi" fn(*mut Self, usize) -> Status,
    pub clear_screen: extern "efiapi" fn(*mut Self) -> Status,
    pub set_cursor_position: extern "efiapi" fn(*mut Self, usize, usize) -> Status,
    pub enable_cursor: extern "efiapi" fn(*mut Self, u8) -> Status,
    pub mode: *mut SimpleTextOutputMode,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

#[repr(C)]
pub struct SimpleTextInput {
    pub reset: extern "efiapi" fn(*mut Self, u8) -> Status,
    pub read_key_stroke: extern "efiapi" fn(*mut Self, *mut InputKey) -> Status,
    pub wait_for_key: Event,
}

#[repr(C)]
pub struct Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub pad1: u8,
    pub nanosecond: u32,
    pub time_zone: i16,
    pub daylight: u8,
    pub pad2: u8,
}

#[repr(C)]
pub struct RuntimeServices {
    pub hdr: TableHeader,
    pub get_time: extern "efiapi" fn(*mut Time, *mut c_void) -> Status,
    pub set_time: usize,
    pub get_wakeup_time: usize,
    pub set_wakeup_time: usize,
    pub set_virtual_address_map: usize,
    pub convert_pointer: usize,
    pub get_variable: usize,
    pub get_next_variable_name: usize,
    pub set_variable: usize,
    pub get_next_high_monotonic_count: usize,
    pub reset_system: extern "efiapi" fn(u32, Status, usize, *mut c_void) -> !,
}

pub const RESET_COLD: u32 = 0;
pub const RESET_SHUTDOWN: u32 = 2;

#[repr(C)]
pub struct MemoryDescriptor {
    pub typ: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,
    pub raise_tpl: usize,
    pub restore_tpl: usize,
    pub allocate_pages: usize,
    pub free_pages: usize,
    pub get_memory_map: extern "efiapi" fn(
        *mut usize,
        *mut MemoryDescriptor,
        *mut usize,
        *mut usize,
        *mut u32,
    ) -> Status,
    pub allocate_pool:
        extern "efiapi" fn(u32, usize, *mut *mut u8) -> Status,
    pub free_pool: extern "efiapi" fn(*mut u8) -> Status,
    pub create_event: extern "efiapi" fn(
        u32,
        usize,
        *mut c_void,
        *mut c_void,
        *mut Event,
    ) -> Status,
    pub set_timer: extern "efiapi" fn(Event, u32, u64) -> Status,
    pub wait_for_event:
        extern "efiapi" fn(usize, *const Event, *mut usize) -> Status,
    pub signal_event: usize,
    pub close_event: usize,
    pub check_event: extern "efiapi" fn(Event) -> Status,
    pub install_protocol_interface: usize,
    pub reinstall_protocol_interface: usize,
    pub uninstall_protocol_interface: usize,
    pub handle_protocol:
        extern "efiapi" fn(Handle, *const Guid, *mut *mut c_void) -> Status,
    pub reserved: usize,
    pub register_protocol_notify: usize,
    pub locate_handle: usize,
    pub locate_device_path: usize,
    pub install_configuration_table: usize,
    pub load_image: usize,
    pub start_image: usize,
    pub exit: usize,
    pub unload_image: usize,
    pub exit_boot_services: usize,
    pub get_next_monotonic_count: usize,
    pub stall: extern "efiapi" fn(usize) -> Status,
    pub set_watchdog_timer:
        extern "efiapi" fn(usize, u64, usize, *mut u16) -> Status,
    pub connect_controller: usize,
    pub disconnect_controller: usize,
    pub open_protocol: usize,
    pub close_protocol: usize,
    pub open_protocol_information: usize,
    pub protocols_per_handle: usize,
    pub locate_handle_buffer: extern "efiapi" fn(
        u32,
        *const Guid,
        *mut c_void,
        *mut usize,
        *mut *mut Handle,
    ) -> Status,
    pub locate_protocol:
        extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
    pub install_multiple_protocol_interfaces: usize,
    pub uninstall_multiple_protocol_interfaces: usize,
    pub calculate_crc32: usize,
    pub copy_mem: usize,
    pub set_mem: usize,
    pub create_event_ex: usize,
}

pub const EVT_TIMER: u32 = 0x8000_0000;
pub const TPL_APPLICATION: usize = 4;
pub const TIMER_PERIODIC: u32 = 1;
pub const MEM_LOADER_DATA: u32 = 2;

// ---- Graphics Output Protocol ----

#[repr(C)]
pub struct GopModeInfo {
    pub version: u32,
    pub h_res: u32,
    pub v_res: u32,
    pub pixel_format: u32,
    pub pixel_info: [u32; 4],
    pub pixels_per_scanline: u32,
}

#[repr(C)]
pub struct GopMode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *mut GopModeInfo,
    pub size_of_info: usize,
    pub framebuffer_base: u64,
    pub framebuffer_size: usize,
}

#[repr(C)]
pub struct Gop {
    pub query_mode:
        extern "efiapi" fn(*mut Self, u32, *mut usize, *mut *mut GopModeInfo) -> Status,
    pub set_mode: extern "efiapi" fn(*mut Self, u32) -> Status,
    pub blt: usize,
    pub mode: *mut GopMode,
}

pub const PIXEL_RGBX: u32 = 0;
pub const PIXEL_BGRX: u32 = 1;

// ---- Loaded Image + Simple File System (to read the BrAIn Key off the ESP) ----

#[repr(C)]
pub struct LoadedImage {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *mut SystemTable,
    pub device_handle: Handle,
    // rest unused
}

#[repr(C)]
pub struct SimpleFileSystem {
    pub revision: u64,
    pub open_volume: extern "efiapi" fn(*mut Self, *mut *mut FileProtocol) -> Status,
}

#[repr(C)]
pub struct FileProtocol {
    pub revision: u64,
    pub open: extern "efiapi" fn(
        *mut Self,
        *mut *mut FileProtocol,
        *const u16,
        u64,
        u64,
    ) -> Status,
    pub close: extern "efiapi" fn(*mut Self) -> Status,
    pub delete: extern "efiapi" fn(*mut Self) -> Status,
    pub read: extern "efiapi" fn(*mut Self, *mut usize, *mut c_void) -> Status,
    pub write: extern "efiapi" fn(*mut Self, *mut usize, *const c_void) -> Status,
    pub get_position: usize,
    pub set_position: extern "efiapi" fn(*mut Self, u64) -> Status,
    pub get_info: usize,
    pub set_info: usize,
    pub flush: extern "efiapi" fn(*mut Self) -> Status,
}

pub const FILE_MODE_READ: u64 = 1;
pub const FILE_MODE_WRITE: u64 = 2;
pub const FILE_MODE_CREATE: u64 = 0x8000_0000_0000_0000;

/// `locate_handle_buffer` search type: every handle carrying a protocol.
pub const BY_PROTOCOL: u32 = 2;

/// EFI_FILE_INFO, as returned one record per `read` on a directory handle.
/// Laid out by hand because the trailing filename is variable-length: the
/// fixed part is 80 bytes, then UCS-2 characters to a NUL.
pub const FILE_INFO_NAME_OFFSET: usize = 80;
pub const FILE_INFO_SIZE_OFFSET: usize = 8;
pub const FILE_INFO_ATTR_OFFSET: usize = 72;
pub const FILE_ATTR_DIRECTORY: u64 = 0x10;
