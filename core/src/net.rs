//! The network reachability probe — diagnostic, KIRA-gated, bounded.
//!
//! This is NOT a limb: the entity gets no verb, cognition cannot invoke
//! it, and nothing here persists anywhere but the boot log. It answers
//! one question with zero ambiguity, once per boot, when (and only when)
//! the firmware offers an HTTP client: does this machine's network
//! actually work — real DHCP lease, real GET, real bytes back?
//!
//! Every step reports its exact outcome and every wait is bounded, so on
//! hardware where any stage fails the report says precisely which stage
//! and with what status — a datum to engineer against, not a shrug.
//!
//! Uses the firmware's own stack (EFI_IP4_CONFIG2 for DHCP policy,
//! EFI_HTTP for the fetch). No packet is hand-rolled here; if the
//! firmware has no stack, the honest report is that it has none.

use crate::console::Ctx;
use crate::efi::*;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::null_mut;

// ---------- FFI shapes (UEFI 2.x NetworkPkg) ----------

#[repr(C)]
struct Ip4Config2 {
    set_data: extern "efiapi" fn(*mut Ip4Config2, u32, usize, *const c_void) -> Status,
    get_data: extern "efiapi" fn(*mut Ip4Config2, u32, *mut usize, *mut c_void) -> Status,
    register_data_notify: usize,
    unregister_data_notify: usize,
}
const IP4C2_INTERFACE_INFO: u32 = 0;
const IP4C2_POLICY: u32 = 1; // Ip4Config2DataTypePolicy (EDK2 enum ordinal)
const IP4C2_POLICY_DHCP: u32 = 1;
/// InterfaceInfo: CHAR16 Name[32] (64) + UINT8 IfType (+3 pad) +
/// UINT32 HwAddressSize + EFI_MAC_ADDRESS (32) → StationAddress at 104.
const IFINFO_STATION_OFF: usize = 104;

#[repr(C)]
struct ServiceBinding {
    create_child: extern "efiapi" fn(*mut ServiceBinding, *mut Handle) -> Status,
    destroy_child: extern "efiapi" fn(*mut ServiceBinding, Handle) -> Status,
}

#[repr(C)]
struct Http {
    get_mode_data: usize,
    configure: extern "efiapi" fn(*mut Http, *const HttpConfigData) -> Status,
    request: extern "efiapi" fn(*mut Http, *mut HttpToken) -> Status,
    cancel: extern "efiapi" fn(*mut Http, *mut c_void) -> Status,
    response: extern "efiapi" fn(*mut Http, *mut HttpToken) -> Status,
    poll: extern "efiapi" fn(*mut Http) -> Status,
}

#[repr(C)]
struct HttpConfigData {
    http_version: u32, // 1 = HTTP/1.1
    timeout_ms: u32,
    local_is_ipv6: u8,
    _pad: [u8; 7],
    access_point: *const HttpV4AccessPoint,
}

#[repr(C)]
struct HttpV4AccessPoint {
    use_default_address: u8,
    local_address: [u8; 4],
    local_subnet: [u8; 4],
    local_port: u16,
}

#[repr(C)]
struct HttpToken {
    event: Event,
    status: Status,
    message: *mut HttpMessage,
}

#[repr(C)]
struct HttpMessage {
    data: *mut c_void, // HttpRequestData* on request, HttpResponseData* on response
    header_count: usize,
    headers: *mut HttpHeader,
    body_length: usize,
    body: *mut c_void,
}

#[repr(C)]
struct HttpRequestData {
    method: u32, // 0 = GET
    _pad: u32,
    url: *const u16,
}

#[repr(C)]
struct HttpResponseData {
    status_code: u32, // EDK2 enum ordinal, NOT the literal HTTP code
}

#[repr(C)]
struct HttpHeader {
    name: *const u8,  // ascii, nul-terminated
    value: *const u8, // ascii, nul-terminated
}

/// EDK2's EFI_HTTP_STATUS_CODE is an ordinal; map the ones that matter.
fn http_code(ord: u32) -> u32 {
    match ord {
        3 => 200,
        4 => 201,
        11 => 301,
        12 => 302,
        16 => 400,
        19 => 403,
        20 => 404,
        26 => 500,
        _ => ord + 9000, // unmapped ordinal, reported recognizably raw
    }
}

const URL: &str = "http://example.com/";

// ---------- shared primitives: one proven path, two callers ----------
//
// The boot probe and the entity's own `web.*` limb must not be two
// different implementations of "fetch a page" — if they diverged, the
// probe would stop being evidence about the limb. Both compose these.

/// The first handle offering an HTTP service binding, if any.
fn first_http_nic(ctx: &Ctx) -> Option<Handle> {
    unsafe {
        let mut count: usize = 0;
        let mut handles: *mut Handle = null_mut();
        if ((*ctx.bs).locate_handle_buffer)(
            BY_PROTOCOL, &HTTP_SB_GUID, null_mut(), &mut count, &mut handles,
        ) != SUCCESS
            || count == 0
        {
            return None;
        }
        let nic = *handles;
        ((*ctx.bs).free_pool)(handles as *mut u8);
        Some(nic)
    }
}

/// Ask the firmware for a DHCP lease. Returns (lease, ms waited); the
/// caller decides how loudly to report it.
fn ensure_dhcp(ctx: &Ctx, nic: Handle) -> (Option<[u8; 4]>, usize) {
    unsafe {
        let bs = ctx.bs;
        let mut ipc: *mut c_void = null_mut();
        if ((*bs).handle_protocol)(nic, &IP4_CONFIG2_GUID, &mut ipc) != SUCCESS {
            return (None, 0);
        }
        let ipc = ipc as *mut Ip4Config2;
        let policy = IP4C2_POLICY_DHCP;
        ((*ipc).set_data)(ipc, IP4C2_POLICY, 4, &policy as *const u32 as *const c_void);
        let mut waited = 0usize;
        while waited < 12_000 {
            let mut buf = [0u8; 200];
            let mut n = buf.len();
            if ((*ipc).get_data)(
                ipc, IP4C2_INTERFACE_INFO, &mut n, buf.as_mut_ptr() as *mut c_void,
            ) == SUCCESS
                && n > IFINFO_STATION_OFF + 4
            {
                let s = &buf[IFINFO_STATION_OFF..IFINFO_STATION_OFF + 4];
                if s != [0, 0, 0, 0] {
                    return (Some([s[0], s[1], s[2], s[3]]), waited);
                }
            }
            ((*bs).stall)(500_000);
            waited += 500;
        }
        (None, waited)
    }
}

/// An open HTTP child. `close()` is mandatory — the firmware leaks the
/// child otherwise, and a leaked child eventually refuses new ones.
struct HttpChild {
    sb: *mut ServiceBinding,
    child: Handle,
    http: *mut Http,
}

impl HttpChild {
    fn close(self) {
        unsafe { ((*self.sb).destroy_child)(self.sb, self.child) };
    }
}

fn open_http(ctx: &Ctx, nic: Handle) -> Result<HttpChild, String> {
    unsafe {
        let bs = ctx.bs;
        let mut sbp: *mut c_void = null_mut();
        if ((*bs).handle_protocol)(nic, &HTTP_SB_GUID, &mut sbp) != SUCCESS {
            return Err(String::from("cannot open http service binding"));
        }
        let sb = sbp as *mut ServiceBinding;
        let mut child: Handle = null_mut();
        let st = ((*sb).create_child)(sb, &mut child);
        if st != SUCCESS {
            return Err(format!("http create_child failed, status {st:#x}"));
        }
        let mut httpp: *mut c_void = null_mut();
        if ((*bs).handle_protocol)(child, &HTTP_GUID, &mut httpp) != SUCCESS {
            ((*sb).destroy_child)(sb, child);
            return Err(String::from("http child lacks http protocol"));
        }
        let http = httpp as *mut Http;
        let ap = HttpV4AccessPoint {
            use_default_address: 1,
            local_address: [0; 4],
            local_subnet: [0; 4],
            local_port: 0,
        };
        let cfg = HttpConfigData {
            http_version: 1,
            timeout_ms: 10_000,
            local_is_ipv6: 0,
            _pad: [0; 7],
            access_point: &ap,
        };
        let st = ((*http).configure)(http, &cfg);
        if st != SUCCESS {
            ((*sb).destroy_child)(sb, child);
            return Err(format!("http configure failed, status {st:#x}"));
        }
        Ok(HttpChild { sb, child, http })
    }
}

/// The host part of a URL — what the Host header must carry. EDK2's HTTP
/// driver requires it explicitly; without it the request is rejected.
fn host_of(url: &str) -> &str {
    let rest = match url.find("://") {
        Some(i) => &url[i + 3..],
        None => url,
    };
    match rest.find('/') {
        Some(i) => &rest[..i],
        None => rest,
    }
}

/// One bounded GET. Returns (http status, bytes written, Location header
/// when the server redirected). Every failure names its stage.
fn do_get(
    ctx: &Ctx,
    c: &HttpChild,
    url: &str,
    out: &mut [u8],
) -> Result<(u32, usize, Option<String>), String> {
    unsafe {
        let bs = ctx.bs;
        let url16 = crate::console::ucs2(url);
        let host_hdr = b"Host\0";
        let host_val = format!("{}\0", host_of(url));
        let mut headers = [HttpHeader {
            name: host_hdr.as_ptr(),
            value: host_val.as_ptr(),
        }];
        let mut reqdata = HttpRequestData { method: 0, _pad: 0, url: url16.as_ptr() };
        let mut reqmsg = HttpMessage {
            data: &mut reqdata as *mut _ as *mut c_void,
            header_count: 1,
            headers: headers.as_mut_ptr(),
            body_length: 0,
            body: null_mut(),
        };
        let mut ev: Event = null_mut();
        if ((*bs).create_event)(0, 0, null_mut(), null_mut(), &mut ev) != SUCCESS {
            return Err(String::from("cannot create completion event"));
        }
        let mut token = HttpToken { event: ev, status: 0, message: &mut reqmsg };
        let st = ((*c.http).request)(c.http, &mut token);
        if st != SUCCESS {
            return Err(format!("GET submit failed, status {st:#x}"));
        }
        if !pump(ctx, c.http, ev, 15_000) {
            return Err(String::from("GET did not complete within 15s"));
        }
        if token.status != SUCCESS {
            return Err(format!(
                "GET completed with error status {:#x} (dns/tcp layer)",
                token.status
            ));
        }

        let mut respdata = HttpResponseData { status_code: 0 };
        let mut respmsg = HttpMessage {
            data: &mut respdata as *mut _ as *mut c_void,
            header_count: 0,
            headers: null_mut(),
            body_length: out.len(),
            body: out.as_mut_ptr() as *mut c_void,
        };
        let mut ev2: Event = null_mut();
        ((*bs).create_event)(0, 0, null_mut(), null_mut(), &mut ev2);
        let mut token2 = HttpToken { event: ev2, status: 0, message: &mut respmsg };
        let st = ((*c.http).response)(c.http, &mut token2);
        if st != SUCCESS || !pump(ctx, c.http, ev2, 15_000) {
            return Err(format!("response not delivered (submit {st:#x}, waited 15s)"));
        }

        // the firmware allocates the response headers; read Location for
        // redirects, then hand the buffer back
        let mut location: Option<String> = None;
        if !respmsg.headers.is_null() {
            for i in 0..respmsg.header_count {
                let h = &*respmsg.headers.add(i);
                if cstr_eq_ignore_case(h.name, b"location") {
                    location = Some(cstr_to_string(h.value, 200));
                }
            }
            ((*bs).free_pool)(respmsg.headers as *mut u8);
        }
        let code = http_code(respdata.status_code);
        let got = respmsg.body_length.min(out.len());
        Ok((code, got, location))
    }
}

/// Wait for `event` while pumping the HTTP driver, bounded.
fn pump(ctx: &Ctx, http: *mut Http, event: Event, ms: usize) -> bool {
    unsafe {
        let bs = ctx.bs;
        let mut waited = 0usize;
        while waited < ms {
            ((*http).poll)(http);
            if ((*bs).check_event)(event) == SUCCESS {
                return true;
            }
            ((*bs).stall)(20_000);
            waited += 20;
        }
        false
    }
}

unsafe fn cstr_eq_ignore_case(p: *const u8, want: &[u8]) -> bool {
    if p.is_null() {
        return false;
    }
    for (i, w) in want.iter().enumerate() {
        let b = *p.add(i);
        if b == 0 || b.to_ascii_lowercase() != *w {
            return false;
        }
    }
    *p.add(want.len()) == 0
}

unsafe fn cstr_to_string(p: *const u8, cap: usize) -> String {
    let mut s = String::new();
    if p.is_null() {
        return s;
    }
    for i in 0..cap {
        let b = *p.add(i);
        if b == 0 {
            break;
        }
        if (0x20..0x7F).contains(&b) {
            s.push(b as char);
        }
    }
    s
}

/// The entity's own fetch (§9.2: the internet as a navigational sense).
/// Follows up to two redirects — an http→https bounce is the common case
/// on the real web, and a real OS follows it rather than reporting a 301.
/// Returns (final url, http status, body bytes).
pub fn fetch(ctx: &Ctx, url: &str, cap: usize) -> Result<(String, u32, Vec<u8>), String> {
    let Some(nic) = first_http_nic(ctx) else {
        return Err(String::from("this body has no network organ right now"));
    };
    ensure_dhcp(ctx, nic); // idempotent; already-leased returns immediately
    let c = open_http(ctx, nic)?;
    let mut current = String::from(url);
    let mut out = vec![0u8; cap];
    for _ in 0..3 {
        match do_get(ctx, &c, &current, &mut out) {
            Err(e) => {
                c.close();
                return Err(e);
            }
            Ok((code, n, location)) => {
                if (code == 301 || code == 302) && location.is_some() {
                    let next = location.unwrap();
                    if !next.is_empty() {
                        current = next;
                        continue;
                    }
                }
                out.truncate(n);
                c.close();
                return Ok((current, code, out));
            }
        }
    }
    c.close();
    Err(String::from("too many redirects"))
}

struct Probe<'a> {
    ctx: &'a Ctx,
    report: Vec<String>,
}

impl<'a> Probe<'a> {
    fn say(&mut self, line: String) {
        self.ctx.color(crate::console::GRAY);
        self.ctx.println(&format!("  [net] {line}"));
        self.report.push(line);
    }

}

/// Run the boot reachability diagnostic. Returns the report lines (also
/// printed and captured into the flight recorder). Call ONLY after KIRA
/// granted `Action::NetProbe`.
///
/// It fetches through the SAME `fetch` the entity's own limb uses, so a
/// green probe is evidence about the limb rather than about a parallel
/// implementation that merely resembles it.
pub fn reachability_probe(ctx: &Ctx) -> Vec<String> {
    let mut p = Probe { ctx, report: Vec::new() };
    let (snp, httpc) = ctx.net_organs();
    p.say(format!("organs: {snp} firmware nic(s), {httpc} http client(s)"));
    if httpc == 0 {
        p.say(String::from(
            "VERDICT: no firmware http client — no reachability possible \
             from the core on this machine",
        ));
        return p.report;
    }
    let Some(nic) = first_http_nic(ctx) else {
        p.say(String::from("VERDICT: http service binding vanished on open"));
        return p.report;
    };
    match ensure_dhcp(ctx, nic) {
        (Some(ip), ms) => p.say(format!(
            "dhcp lease: {}.{}.{}.{} ({ms} ms)",
            ip[0], ip[1], ip[2], ip[3]
        )),
        (None, ms) => p.say(format!(
            "dhcp: NO lease within {ms}ms (cable/AP? firmware NIC driver?) — \
             continuing, the fetch will show the consequence"
        )),
    }
    match fetch(ctx, URL, 600) {
        Err(e) => p.say(format!("VERDICT: {e}")),
        Ok((final_url, code, body)) => {
            let head: String = body
                .iter()
                .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { ' ' })
                .take(60)
                .collect();
            let via = if final_url == URL {
                String::new()
            } else {
                format!(" (redirected to {final_url})")
            };
            p.say(format!(
                "VERDICT: GET {URL}{via} -> HTTP {code}, {} bytes received; \
                 first bytes: {}",
                body.len(),
                head.trim()
            ));
        }
    }
    p.report
}
