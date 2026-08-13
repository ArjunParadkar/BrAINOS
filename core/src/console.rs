//! Embodiment plumbing for the laptop body: text console, framebuffer,
//! clock, RAM probe, and files on the key medium. This is the thin layer
//! the body-map regions act through; cognition never calls EFI directly.

use crate::efi::*;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::{null_mut, write_volatile};

/// Marker file that identifies the world volume. Presence of this file is
/// what makes a disk "the world" — never enumeration order.
pub const WORLD_MARKER: &str = "WORLD.ID";

// ---------- the flight recorder ----------
//
// Real laptops have no COM1, so the QEMU serial mirror does not exist on
// metal. Everything printed is also appended here and flushed to
// \BRAIN\BOOT.LOG on the KEY (write_file -> open_root -> the boot device,
// never anywhere else) at boot milestones — so a hang, a surprise from
// the network probe, or a limb failure leaves a real log to read on
// another machine afterwards, not a memory of what flashed by.
// NOTE: a hang BEFORE the core runs (firmware-level) can never be
// captured by the core; the log proves how far the core got.

const BOOTLOG_CAP: usize = 24 * 1024;
static mut BOOTLOG: [u8; BOOTLOG_CAP] = [0; BOOTLOG_CAP];
static BOOTLOG_LEN: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

fn bootlog_push(bytes: &[u8]) {
    use core::sync::atomic::Ordering;
    let len = BOOTLOG_LEN.load(Ordering::Relaxed);
    if len >= BOOTLOG_CAP {
        return; // full: keep the earliest history, it holds the boot story
    }
    let n = bytes.len().min(BOOTLOG_CAP - len);
    unsafe {
        let base = (&raw mut BOOTLOG) as *mut u8;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(len), n);
    }
    BOOTLOG_LEN.store(len + n, Ordering::Relaxed);
}

// ---------- the screen as a sense, not only an effector (§9.1) ----------
//
// The boot log deliberately keeps the EARLIEST bytes (it is a flight
// recorder for a hang). Proprioception needs the opposite: what is on the
// screen NOW. So the rendered text also feeds a true ring that overwrites
// oldest-first, and `screen_text` reconstructs the visible tail from it.

const SCREEN_CAP: usize = 4096;
static mut SCREEN_RING: [u8; SCREEN_CAP] = [0; SCREEN_CAP];
/// total bytes ever rendered; the ring holds the last SCREEN_CAP of them
static SCREEN_WRITTEN: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

fn screen_push(bytes: &[u8]) {
    use core::sync::atomic::Ordering;
    let mut w = SCREEN_WRITTEN.load(Ordering::Relaxed);
    unsafe {
        let base = (&raw mut SCREEN_RING) as *mut u8;
        for &b in bytes {
            *base.add(w % SCREEN_CAP) = b;
            w += 1;
        }
    }
    SCREEN_WRITTEN.store(w, Ordering::Relaxed);
}

/// The text currently standing on the screen, oldest-to-newest, with
/// carriage-return overwrites resolved the way a terminal resolves them
/// (the status line the loop keeps rewriting shows once, not N times).
/// Returns at most `max_lines` lines from the bottom.
pub fn screen_text(max_lines: usize) -> Vec<String> {
    use core::sync::atomic::Ordering;
    let w = SCREEN_WRITTEN.load(Ordering::Relaxed);
    let n = w.min(SCREEN_CAP);
    let start = w - n;
    let mut raw: Vec<u8> = Vec::with_capacity(n);
    unsafe {
        let base = (&raw const SCREEN_RING) as *const u8;
        for i in 0..n {
            raw.push(*base.add((start + i) % SCREEN_CAP));
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for &b in &raw {
        match b {
            b'\n' => {
                lines.push(core::mem::take(&mut cur));
            }
            // a carriage return means the rest overwrote this line
            b'\r' => cur.clear(),
            0x20..=0x7E => cur.push(b as char),
            _ => {}
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    // the first line may be a fragment the ring cut in half; drop it once
    // the ring has wrapped, rather than reporting a half-truth
    if w > SCREEN_CAP && !lines.is_empty() {
        lines.remove(0);
    }
    lines.retain(|l| !l.trim().is_empty());
    let keep = lines.len().saturating_sub(max_lines);
    lines.split_off(keep)
}

// Text attribute colors (EFI console palette)
pub const DIM: usize = 0x08;
pub const GRAY: usize = 0x07;
pub const WHITE: usize = 0x0F;
pub const AMBER: usize = 0x0E;
pub const GREEN: usize = 0x0A;
pub const CYAN: usize = 0x0B;
pub const RED: usize = 0x0C;

pub struct Ctx {
    pub st: *mut SystemTable,
    pub out: *mut SimpleTextOutput,
    pub input: *mut SimpleTextInput,
    pub bs: *mut BootServices,
    pub rs: *mut RuntimeServices,
    pub image: Handle,
}

pub fn ucs2(path: &str) -> Vec<u16> {
    let mut v: Vec<u16> = path.bytes().map(|b| b as u16).collect();
    v.push(0);
    v
}

impl Ctx {
    pub fn color(&self, attr: usize) {
        unsafe { ((*self.out).set_attribute)(self.out, attr) };
    }

    /// Print ASCII as UCS-2, translating '\n' to CRLF.
    pub fn print(&self, s: &str) {
        bootlog_push(s.as_bytes());
        screen_push(s.as_bytes());
        let mut buf = [0u16; 160];
        let mut i = 0;
        for b in s.bytes() {
            if i >= buf.len() - 3 {
                buf[i] = 0;
                unsafe { ((*self.out).output_string)(self.out, buf.as_ptr()) };
                i = 0;
            }
            if b == b'\n' {
                buf[i] = 0x0D;
                buf[i + 1] = 0x0A;
                i += 2;
            } else if b == b'\r' || (0x20..0x7F).contains(&b) {
                buf[i] = b as u16;
                i += 1;
            }
        }
        buf[i] = 0;
        unsafe { ((*self.out).output_string)(self.out, buf.as_ptr()) };
    }

    pub fn println(&self, s: &str) {
        self.print(s);
        self.print("\n");
    }

    pub fn sleep_ms(&self, ms: usize) {
        unsafe { ((*self.bs).stall)(ms * 1000) };
    }

    /// Write the flight recorder to \BRAIN\BOOT.LOG on the key. Cheap
    /// enough to call at every milestone; the last flush wins.
    pub fn flush_bootlog(&self) {
        use core::sync::atomic::Ordering;
        let len = BOOTLOG_LEN.load(Ordering::Relaxed);
        if len == 0 {
            return;
        }
        let mut snap = alloc::vec![0u8; len];
        unsafe {
            let base = (&raw const BOOTLOG) as *const u8;
            core::ptr::copy_nonoverlapping(base, snap.as_mut_ptr(), len);
        }
        self.write_file("BRAIN\\BOOT.LOG", &snap);
    }

    /// The entity speaking: typed out character by character.
    pub fn speak(&self, s: &str, per_char_ms: usize) {
        bootlog_push(s.as_bytes());
        screen_push(s.as_bytes());
        for b in s.bytes() {
            if (0x20..0x7F).contains(&b) {
                let buf = [b as u16, 0u16];
                unsafe { ((*self.out).output_string)(self.out, buf.as_ptr()) };
                self.sleep_ms(per_char_ms);
            }
        }
    }

    pub fn set_cursor(&self, col: usize, row: usize) {
        unsafe { ((*self.out).set_cursor_position)(self.out, col, row) };
    }

    /// Where the firmware thinks the cursor is. The line editor needs this
    /// to redraw an edited line in place without disturbing the transcript
    /// scrolled above it.
    pub fn cursor_col(&self) -> usize {
        unsafe { (*(*self.out).mode).cursor_column.max(0) as usize }
    }

    pub fn cursor_row(&self) -> usize {
        unsafe { (*(*self.out).mode).cursor_row.max(0) as usize }
    }

    pub fn show_cursor(&self, on: bool) {
        unsafe { ((*self.out).enable_cursor)(self.out, on as u8) };
    }

    /// Console width in columns (mode 0 is 80x25 on every firmware we
    /// target; query anyway so a wider mode edits correctly).
    pub fn cols(&self) -> usize {
        unsafe {
            let m = (*self.out).mode;
            let mut c: usize = 0;
            let mut r: usize = 0;
            if ((*self.out).query_mode)(self.out, (*m).mode.max(0) as usize, &mut c, &mut r)
                == SUCCESS
                && c > 0
            {
                c
            } else {
                80
            }
        }
    }

    pub fn rows(&self) -> usize {
        unsafe {
            let m = (*self.out).mode;
            let mut c: usize = 0;
            let mut r: usize = 0;
            if ((*self.out).query_mode)(self.out, (*m).mode.max(0) as usize, &mut c, &mut r)
                == SUCCESS
                && r > 0
            {
                r
            } else {
                25
            }
        }
    }

    /// Non-blocking key read.
    pub fn poll_key(&self) -> Option<InputKey> {
        let mut k = InputKey::default();
        let s = unsafe { ((*self.input).read_key_stroke)(self.input, &mut k) };
        if s == SUCCESS {
            Some(k)
        } else {
            None
        }
    }

    pub fn now(&self) -> Time {
        let mut t = Time {
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            pad1: 0,
            nanosecond: 0,
            time_zone: 0,
            daylight: 0,
            pad2: 0,
        };
        unsafe { ((*self.rs).get_time)(&mut t, null_mut()) };
        t
    }

    /// Total RAM visible in the UEFI memory map, in MiB.
    pub fn ram_mib(&self) -> u64 {
        unsafe {
            let mut size: usize = 0;
            let mut map_key: usize = 0;
            let mut desc_size: usize = 0;
            let mut ver: u32 = 0;
            ((*self.bs).get_memory_map)(
                &mut size,
                null_mut(),
                &mut map_key,
                &mut desc_size,
                &mut ver,
            );
            size += 4096;
            let mut buf: *mut u8 = null_mut();
            if ((*self.bs).allocate_pool)(MEM_LOADER_DATA, size, &mut buf) != SUCCESS {
                return 0;
            }
            let st = ((*self.bs).get_memory_map)(
                &mut size,
                buf as *mut MemoryDescriptor,
                &mut map_key,
                &mut desc_size,
                &mut ver,
            );
            let mut pages: u64 = 0;
            if st == SUCCESS && desc_size > 0 {
                for i in 0..size / desc_size {
                    let d = &*(buf.add(i * desc_size) as *const MemoryDescriptor);
                    if d.typ >= 1 && d.typ <= 10 {
                        pages += d.number_of_pages;
                    }
                }
            }
            ((*self.bs).free_pool)(buf);
            pages * 4096 / (1024 * 1024)
        }
    }

    pub fn gop(&self) -> Option<&'static mut Gop> {
        unsafe {
            let mut p: *mut c_void = null_mut();
            let s = ((*self.bs).locate_protocol)(&GOP_GUID, null_mut(), &mut p);
            if s == SUCCESS && !p.is_null() {
                Some(&mut *(p as *mut Gop))
            } else {
                None
            }
        }
    }

    /// Count handles carrying one protocol — pure discovery, opens nothing.
    fn count_handles(&self, guid: &Guid) -> usize {
        unsafe {
            let mut count: usize = 0;
            let mut handles: *mut Handle = null_mut();
            if ((*self.bs).locate_handle_buffer)(
                BY_PROTOCOL,
                guid,
                null_mut(),
                &mut count,
                &mut handles,
            ) != SUCCESS
            {
                return 0;
            }
            ((*self.bs).free_pool)(handles as *mut u8);
            count
        }
    }

    /// §8 discovery for the network organ: does the firmware offer a NIC
    /// (Simple Network Protocol) and/or its own HTTP client? This only
    /// LOOKS — no protocol is opened, no packet moves. In the contained VM
    /// (-nic none) both counts are zero and that is the honest answer.
    pub fn net_organs(&self) -> (usize, usize) {
        (self.count_handles(&SNP_GUID), self.count_handles(&HTTP_SB_GUID))
    }

    fn open_root(&self) -> Option<*mut FileProtocol> {
        unsafe {
            let mut li: *mut c_void = null_mut();
            if ((*self.bs).handle_protocol)(self.image, &LOADED_IMAGE_GUID, &mut li) != SUCCESS {
                return None;
            }
            let dev = (*(li as *mut LoadedImage)).device_handle;
            let mut fsp: *mut c_void = null_mut();
            if ((*self.bs).handle_protocol)(dev, &SIMPLE_FS_GUID, &mut fsp) != SUCCESS {
                return None;
            }
            let fs = fsp as *mut SimpleFileSystem;
            let mut root: *mut FileProtocol = null_mut();
            if ((*fs).open_volume)(fs, &mut root) != SUCCESS {
                return None;
            }
            Some(root)
        }
    }

    /// Read a file from the key medium the core booted from.
    pub fn read_file(&self, path: &str, buf: &mut [u8]) -> Option<usize> {
        unsafe {
            let root = self.open_root()?;
            let p = ucs2(path);
            let mut f: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut f, p.as_ptr(), FILE_MODE_READ, 0) != SUCCESS {
                ((*root).close)(root);
                return None;
            }
            let mut n = buf.len();
            let st = ((*f).read)(f, &mut n, buf.as_mut_ptr() as *mut c_void);
            ((*f).close)(f);
            ((*root).close)(root);
            if st == SUCCESS {
                Some(n)
            } else {
                None
            }
        }
    }

    /// The world volume: a second disk that is emphatically *not* the key.
    ///
    /// The key carries identity and memory; this carries the entity's
    /// files. Keeping them on separate media is the §13.2 memory-integrity
    /// split made physical — a file the entity writes can never grow into
    /// the region its episodic memory lives in.
    ///
    /// Identified by a marker file rather than by enumeration order, so
    /// attaching disks in a different order cannot silently repoint the
    /// filesystem limb at the wrong volume.
    fn world_root(&self) -> Option<*mut FileProtocol> {
        unsafe {
            let mut li: *mut c_void = null_mut();
            let boot_dev =
                if ((*self.bs).handle_protocol)(self.image, &LOADED_IMAGE_GUID, &mut li)
                    == SUCCESS
                {
                    (*(li as *mut LoadedImage)).device_handle
                } else {
                    null_mut()
                };

            let mut count: usize = 0;
            let mut handles: *mut Handle = null_mut();
            if ((*self.bs).locate_handle_buffer)(
                BY_PROTOCOL,
                &SIMPLE_FS_GUID,
                null_mut(),
                &mut count,
                &mut handles,
            ) != SUCCESS
            {
                return None;
            }

            let mut found: Option<*mut FileProtocol> = None;
            for i in 0..count {
                let h = *handles.add(i);
                if h == boot_dev {
                    continue;
                }
                let mut fsp: *mut c_void = null_mut();
                if ((*self.bs).handle_protocol)(h, &SIMPLE_FS_GUID, &mut fsp) != SUCCESS {
                    continue;
                }
                let fs = fsp as *mut SimpleFileSystem;
                let mut root: *mut FileProtocol = null_mut();
                if ((*fs).open_volume)(fs, &mut root) != SUCCESS {
                    continue;
                }
                // the marker proves this is the world disk — and not by name
                // alone: on real hardware other volumes (the machine's own
                // internal ESP, a stray USB stick) are scanned here too, so
                // a file merely CALLED WORLD.ID must not be enough to aim
                // the filesystem limb (and its writes) at a stranger's disk.
                // The content has to say what only make_world.py writes.
                let marker = ucs2(WORLD_MARKER);
                let mut m: *mut FileProtocol = null_mut();
                if ((*root).open)(root, &mut m, marker.as_ptr(), FILE_MODE_READ, 0) == SUCCESS {
                    let mut head = [0u8; 32];
                    let mut n = head.len();
                    let ok = ((*m).read)(m, &mut n, head.as_mut_ptr() as *mut c_void)
                        == SUCCESS
                        && head[..n].starts_with(b"BrAInOS world volume");
                    ((*m).close)(m);
                    if ok {
                        found = Some(root);
                        break;
                    }
                }
                ((*root).close)(root);
            }
            ((*self.bs).free_pool)(handles as *mut u8);
            found
        }
    }

    /// True when a world disk is attached — the filesystem region is only
    /// incorporated into the body map when this holds.
    pub fn world_present(&self) -> bool {
        unsafe {
            match self.world_root() {
                Some(root) => {
                    ((*root).close)(root);
                    true
                }
                None => false,
            }
        }
    }

    /// List one directory of the world volume. `dir` is "" for the root.
    /// Returns (name, is_directory, byte size) per entry.
    pub fn world_list(&self, dir: &str) -> Option<Vec<(Vec<u8>, bool, u64)>> {
        unsafe {
            let root = self.world_root()?;
            let handle = if dir.is_empty() || dir == "\\" {
                root
            } else {
                let p = ucs2(dir);
                let mut d: *mut FileProtocol = null_mut();
                if ((*root).open)(root, &mut d, p.as_ptr(), FILE_MODE_READ, 0) != SUCCESS {
                    ((*root).close)(root);
                    return None;
                }
                d
            };

            let mut out: Vec<(Vec<u8>, bool, u64)> = Vec::new();
            let mut rec = [0u8; 512];
            loop {
                let mut n = rec.len();
                if ((*handle).read)(handle, &mut n, rec.as_mut_ptr() as *mut c_void) != SUCCESS {
                    break;
                }
                if n == 0 {
                    break; // end of directory
                }
                if n < FILE_INFO_NAME_OFFSET + 2 {
                    continue;
                }
                let size = u64::from_le_bytes(
                    rec[FILE_INFO_SIZE_OFFSET..FILE_INFO_SIZE_OFFSET + 8]
                        .try_into()
                        .unwrap_or([0; 8]),
                );
                let attr = u64::from_le_bytes(
                    rec[FILE_INFO_ATTR_OFFSET..FILE_INFO_ATTR_OFFSET + 8]
                        .try_into()
                        .unwrap_or([0; 8]),
                );
                // trailing UCS-2 name, narrowed to ASCII
                let mut name: Vec<u8> = Vec::new();
                let mut i = FILE_INFO_NAME_OFFSET;
                while i + 1 < n {
                    let c = u16::from_le_bytes([rec[i], rec[i + 1]]);
                    if c == 0 {
                        break;
                    }
                    if (0x20..0x7F).contains(&c) {
                        name.push(c as u8);
                    }
                    i += 2;
                }
                if name.is_empty() || name == b"." || name == b".." {
                    continue;
                }
                out.push((name, attr & FILE_ATTR_DIRECTORY != 0, size));
            }

            if handle != root {
                ((*handle).close)(handle);
            }
            ((*root).close)(root);
            Some(out)
        }
    }

    pub fn world_read(&self, path: &str, buf: &mut [u8]) -> Option<usize> {
        self.world_read_at(path, 0, buf)
    }

    /// Read a window of a world file starting at `offset` — large files
    /// cross the boundary as bounded chunks, never loaded whole.
    pub fn world_read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Option<usize> {
        unsafe {
            let root = self.world_root()?;
            let p = ucs2(path);
            let mut f: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut f, p.as_ptr(), FILE_MODE_READ, 0) != SUCCESS {
                ((*root).close)(root);
                return None;
            }
            if offset > 0 && ((*f).set_position)(f, offset) != SUCCESS {
                ((*f).close)(f);
                ((*root).close)(root);
                return None;
            }
            let mut n = buf.len();
            let st = ((*f).read)(f, &mut n, buf.as_mut_ptr() as *mut c_void);
            ((*f).close)(f);
            ((*root).close)(root);
            if st == SUCCESS {
                Some(n)
            } else {
                None
            }
        }
    }

    /// Create a directory on the world volume. Parent must already exist —
    /// the caller builds nested paths one honest level at a time.
    pub fn world_mkdir(&self, path: &str) -> bool {
        unsafe {
            let Some(root) = self.world_root() else { return false };
            let p = ucs2(path);
            let rw = FILE_MODE_READ | FILE_MODE_WRITE;
            let mut d: *mut FileProtocol = null_mut();
            let st = ((*root).open)(
                root,
                &mut d,
                p.as_ptr(),
                rw | FILE_MODE_CREATE,
                FILE_ATTR_DIRECTORY,
            );
            if st == SUCCESS {
                ((*d).close)(d);
            }
            ((*root).close)(root);
            st == SUCCESS
        }
    }

    /// Delete a file (or an empty directory) from the world volume.
    /// The firmware refuses to delete a non-empty directory; that refusal
    /// surfaces as an honest `false`, never a silent recursive wipe.
    pub fn world_delete(&self, path: &str) -> bool {
        unsafe {
            let Some(root) = self.world_root() else { return false };
            let p = ucs2(path);
            let rw = FILE_MODE_READ | FILE_MODE_WRITE;
            let mut f: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut f, p.as_ptr(), rw, 0) != SUCCESS {
                ((*root).close)(root);
                return false;
            }
            // delete() always closes the handle; SUCCESS means it's gone
            let st = ((*f).delete)(f);
            ((*root).close)(root);
            st == SUCCESS
        }
    }

    /// Move/rename within the world volume: chunked copy, verify the byte
    /// count, then delete the source. Never loads the file whole.
    pub fn world_move(&self, src: &str, dst: &str) -> bool {
        unsafe {
            let Some(root) = self.world_root() else { return false };
            let ps = ucs2(src);
            let mut fsrc: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut fsrc, ps.as_ptr(), FILE_MODE_READ, 0) != SUCCESS {
                ((*root).close)(root);
                return false;
            }
            let pd = ucs2(dst);
            let rw = FILE_MODE_READ | FILE_MODE_WRITE;
            // replace any previous file at the destination
            let mut old: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut old, pd.as_ptr(), rw, 0) == SUCCESS {
                ((*old).delete)(old);
            }
            let mut fdst: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut fdst, pd.as_ptr(), rw | FILE_MODE_CREATE, 0) != SUCCESS {
                ((*fsrc).close)(fsrc);
                ((*root).close)(root);
                return false;
            }
            let mut chunk = [0u8; 4096];
            let mut ok = true;
            loop {
                let mut n = chunk.len();
                if ((*fsrc).read)(fsrc, &mut n, chunk.as_mut_ptr() as *mut c_void) != SUCCESS {
                    ok = false;
                    break;
                }
                if n == 0 {
                    break;
                }
                let mut w = n;
                if ((*fdst).write)(fdst, &mut w, chunk.as_ptr() as *const c_void) != SUCCESS
                    || w != n
                {
                    ok = false;
                    break;
                }
            }
            ((*fdst).flush)(fdst);
            ((*fdst).close)(fdst);
            ((*fsrc).close)(fsrc);
            if ok {
                // source goes only after the copy landed whole
                let mut f: *mut FileProtocol = null_mut();
                if ((*root).open)(root, &mut f, ps.as_ptr(), rw, 0) == SUCCESS {
                    ok = ((*f).delete)(f) == SUCCESS;
                } else {
                    ok = false;
                }
            }
            ((*root).close)(root);
            ok
        }
    }

    pub fn world_write(&self, path: &str, data: &[u8]) -> bool {
        unsafe {
            let Some(root) = self.world_root() else { return false };
            let p = ucs2(path);
            let rw = FILE_MODE_READ | FILE_MODE_WRITE;
            let mut old: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut old, p.as_ptr(), rw, 0) == SUCCESS {
                ((*old).delete)(old); // delete() closes the handle
            }
            let mut f: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut f, p.as_ptr(), rw | FILE_MODE_CREATE, 0) != SUCCESS {
                ((*root).close)(root);
                return false;
            }
            let mut n = data.len();
            let st = ((*f).write)(f, &mut n, data.as_ptr() as *const c_void);
            ((*f).flush)(f);
            ((*f).close)(f);
            ((*root).close)(root);
            st == SUCCESS && n == data.len()
        }
    }

    /// Write (replace) a file on the key medium. Returns false when the
    /// firmware's FAT driver is read-only — the caller degrades gracefully.
    pub fn write_file(&self, path: &str, data: &[u8]) -> bool {
        unsafe {
            let Some(root) = self.open_root() else { return false };
            let p = ucs2(path);
            let rw = FILE_MODE_READ | FILE_MODE_WRITE;
            // replace: delete any previous version, then create fresh
            let mut old: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut old, p.as_ptr(), rw, 0) == SUCCESS {
                ((*old).delete)(old);
            }
            let mut f: *mut FileProtocol = null_mut();
            if ((*root).open)(root, &mut f, p.as_ptr(), rw | FILE_MODE_CREATE, 0) != SUCCESS {
                ((*root).close)(root);
                return false;
            }
            let mut n = data.len();
            let st = ((*f).write)(f, &mut n, data.as_ptr() as *const c_void);
            ((*f).flush)(f);
            ((*f).close)(f);
            ((*root).close)(root);
            st == SUCCESS && n == data.len()
        }
    }
}

// ---------- framebuffer ----------

pub struct Fb {
    pub base: *mut u32,
    pub w: usize,
    pub h: usize,
    pub stride: usize,
    pub format: u32,
}

impl Fb {
    pub fn from(gop: &Gop) -> Option<Fb> {
        unsafe {
            let mode = &*gop.mode;
            let info = &*mode.info;
            if info.pixel_format != PIXEL_RGBX && info.pixel_format != PIXEL_BGRX {
                return None;
            }
            Some(Fb {
                base: mode.framebuffer_base as *mut u32,
                w: info.h_res as usize,
                h: info.v_res as usize,
                stride: info.pixels_per_scanline as usize,
                format: info.pixel_format,
            })
        }
    }

    pub fn pack(&self, r: u32, g: u32, b: u32) -> u32 {
        if self.format == PIXEL_BGRX {
            (r << 16) | (g << 8) | b
        } else {
            (b << 16) | (g << 8) | r
        }
    }

    pub fn fill_row(&self, y: usize, px: u32) {
        unsafe {
            let row = self.base.add(y * self.stride);
            for x in 0..self.w {
                write_volatile(row.add(x), px);
            }
        }
    }

    pub fn rect(&self, x0: usize, y0: usize, w: usize, h: usize, px: u32) {
        unsafe {
            for y in y0..(y0 + h).min(self.h) {
                let row = self.base.add(y * self.stride);
                for x in x0..(x0 + w).min(self.w) {
                    write_volatile(row.add(x), px);
                }
            }
        }
    }

    pub fn put(&self, x: usize, y: usize, px: u32) {
        if x < self.w && y < self.h {
            unsafe { write_volatile(self.base.add(y * self.stride + x), px) }
        }
    }
}
