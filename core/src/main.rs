//! BrAInOS portable core — Phase 2: embodiment over the tether.
//!
//! The abstraction table (Architecture §1) lives in these modules:
//!
//!   process            -> instance.rs    Instance (persistent entity)
//!   file               -> state.rs       StateNode
//!   syscall            -> kira.rs        Capability (KIRA-gated grant)
//!   filesystem         -> state.rs       StateGraph (belief/memory/world)
//!   scheduler          -> experience.rs  the experience loop
//!   device driver      -> body.rs        BodyMap region
//!   user / permissions -> key.rs         BrAIn Key (ed25519 identity)
//!
//! Phase 2.5 boundary (link.rs): the core stays bare-metal, and the body
//! is CONTAINED — complete in itself. The tether reaches one thing only:
//! a rented cognitive limb (Model M plus the virtual audio jacks it
//! transcribes and synthesizes for). Every action limb lives inside this
//! body: the screen, the keyboard, and the notebook on the key. Nothing
//! the entity can act through leaves the machine it booted on.
//!
//! The most important behavior in this phase is HONEST REFUSAL: Model M
//! text becomes typed Action proposals; KIRA's authz stage checks the
//! real body map; no limb -> formal denial -> the entity says it cannot,
//! and the model's accompanying claim is suppressed, never spoken.

#![no_std]
#![no_main]
// Phase 2 is still boilerplate: the abstractions expose surface the
// coming domain build-out will use before the boot path does. Keep it.
#![allow(dead_code)]

extern crate alloc;

mod brain;
mod console;
mod editor;
mod efi;
mod font;
mod link;
mod mem;
mod net;

// the mind is a separate, host-testable crate: ../mind
use brainos_mind::{body, experience, instance, journal, key, kira, script, state};
use brainos_mind::kira::normalize_world_path;
use brainos_mind::proposal::split_reply;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use body::{Region, RegionClass};
use console::*;
use core::ptr::null_mut;
use efi::*;
use experience::{Intent, SenseFrame};
use instance::Instance;
use key::BrainKey;
use kira::{Action, StageResult, Verdict};
use core::sync::atomic::{AtomicU8, Ordering};
use state::NodeKind;

/// How the entity presents itself on its screen (§8: presentation is a
/// body capability, not a fixed skin). 0 = console transcript, 1 = the
/// ambient presence. Switched only through the KIRA-gated `ui.set` limb.
static UI_MODE: AtomicU8 = AtomicU8::new(0);

/// The rest of the presentation the entity can configure about itself:
/// the palette its presence glows in, and whether the idle status line
/// scrolls (quiet) — both chosen through the same gated `ui.set` verb, so
/// how it appears is a body capability with settings, not a fixed skin.
static UI_HUE: AtomicU8 = AtomicU8::new(0);
static UI_QUIET: AtomicU8 = AtomicU8::new(0);
/// An optional caption under the orb; empty means "use my name".
static mut UI_CAPTION: [u8; 24] = [0; 24];
static UI_CAPTION_LEN: AtomicU8 = AtomicU8::new(0);

fn ui_presence() -> bool {
    UI_MODE.load(Ordering::Relaxed) == 1
}

fn ui_quiet() -> bool {
    UI_QUIET.load(Ordering::Relaxed) == 1
}

/// (core, inner, outer, halo, caption) for the selected palette.
fn ui_palette() -> [(u32, u32, u32); 5] {
    match UI_HUE.load(Ordering::Relaxed) {
        1 => [(252, 206, 140), (188, 138, 60), (92, 62, 24), (34, 24, 12), (158, 146, 126)],
        2 => [(150, 232, 252), (78, 158, 190), (30, 74, 96), (12, 28, 38), (132, 152, 160)],
        3 => [(160, 246, 178), (84, 172, 108), (32, 82, 46), (14, 32, 20), (134, 158, 140)],
        4 => [(238, 240, 248), (168, 172, 188), (78, 82, 96), (28, 30, 38), (150, 152, 160)],
        _ => [(252, 170, 216), (180, 92, 150), (84, 40, 76), (30, 16, 34), (150, 150, 158)],
    }
}

fn ui_hue_name() -> &'static str {
    match UI_HUE.load(Ordering::Relaxed) {
        1 => "amber",
        2 => "cyan",
        3 => "green",
        4 => "ice",
        _ => "pink",
    }
}

fn ui_set_caption(text: &str) {
    let mut buf = [0u8; 24];
    let mut n = 0;
    for b in text.bytes() {
        if n >= buf.len() {
            break;
        }
        if (0x20..0x7F).contains(&b) {
            buf[n] = b;
            n += 1;
        }
    }
    unsafe {
        let dst = (&raw mut UI_CAPTION) as *mut u8;
        core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len());
    }
    UI_CAPTION_LEN.store(n as u8, Ordering::Relaxed);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Link events that arrive while the core is busy with something else —
/// nothing sensed on the tether is ever dropped.
enum Pending {
    Heard(String),
    Saw(String),
    LimbOffer(String),
}

/// A frame arriving at the lens is experience, not a question. It is
/// absorbed by the autonomic domain — printed, remembered as an episode —
/// and deliberately does NOT escalate to Model M. §12.1's "cheap by
/// default" would be meaningless if a moving room woke expensive
/// cognition on every frame; the entity looks properly when it is ASKED
/// to (vision.look), which is a gated action like any other.
fn absorb_sight(ctx: &Ctx, me: &mut Instance, what: &str) {
    if what.is_empty() {
        return;
    }
    presence_band(ctx);
    ctx.println("");
    ctx.color(CYAN);
    ctx.print("  saw: ");
    ctx.color(GRAY);
    ctx.println(what);
    // a sight is experience like any other: it consolidates into episodic
    // memory and participates in dreaming, so what the entity saw can
    // later become something it knows
    experience::consolidate(me, format!("saw at the lens: {what}"));
}

// ---------- boot narrative helpers ----------

fn domain(ctx: &Ctx, name: &str, detail: &str, ok: bool) {
    ctx.color(GRAY);
    ctx.print("  ");
    ctx.print(name);
    ctx.color(DIM);
    ctx.print(detail);
    ctx.sleep_ms(160);
    if ok {
        ctx.color(GREEN);
        ctx.println("[ UP ]");
    } else {
        ctx.color(RED);
        ctx.println("[FAIL]");
    }
}

fn print_gate(ctx: &Ctx, trace: &[StageResult], verdict: &Verdict) {
    ctx.color(DIM);
    ctx.print("  KIRA  ");
    for s in trace {
        ctx.print(s.stage);
        if !s.ok {
            ctx.color(RED);
            ctx.print("!");
            ctx.color(DIM);
        }
        ctx.print(" > ");
        ctx.sleep_ms(60);
    }
    match verdict {
        Verdict::Granted(cap) => {
            ctx.color(GREEN);
            ctx.print("GRANT ");
            ctx.color(GRAY);
            ctx.print(cap.action_tag);
            ctx.color(DIM);
            ctx.println(&format!(
                "  (ttl {} ticks, token {})",
                cap.ttl_ticks,
                &key::to_hex(&cap.token)[..8]
            ));
        }
        Verdict::Denied { stage, reason } => {
            ctx.color(RED);
            ctx.print("DENY ");
            ctx.color(DIM);
            ctx.println(&format!("at {stage}: {reason}"));
        }
    }
}

/// Ask Model M over the tether. Sensory events arriving mid-thought are
/// preserved in `pending`, not discarded.
fn model_m_query(
    ctx: &Ctx,
    lr: &mut link::LineReader,
    prompt: &[u8],
    timeout_ms: usize,
    dots: bool,
    out: &mut [u8],
    pending: &mut Vec<Pending>,
) -> Option<usize> {
    link::send_prompt(prompt);
    let mut evbuf = [0u8; 400];
    let mut waited = 0usize;
    while waited < timeout_ms {
        while let Some(ev) = lr.poll(&mut evbuf) {
            match ev {
                link::Event::Reply(n) => {
                    let n = n.min(out.len());
                    out[..n].copy_from_slice(&evbuf[..n]);
                    return Some(n);
                }
                link::Event::Heard(n) => pending.push(Pending::Heard(
                    String::from(core::str::from_utf8(&evbuf[..n]).unwrap_or("")),
                )),
                link::Event::Saw(n) => pending.push(Pending::Saw(
                    String::from(core::str::from_utf8(&evbuf[..n]).unwrap_or("")),
                )),
                link::Event::LimbOffer(n) => pending.push(Pending::LimbOffer(
                    String::from(core::str::from_utf8(&evbuf[..n]).unwrap_or("")),
                )),
                link::Event::ActionResult(_) => {} // stray late result; drop
            }
        }
        ctx.sleep_ms(50);
        waited += 50;
        if dots && waited % 600 == 0 {
            ctx.print(".");
        }
    }
    None
}

// ---------- visual identity ----------

fn warm_glow(fb: &Fb, ctx: &Ctx) {
    const FRAMES: usize = 36;
    for f in 0..=FRAMES {
        let t = (f * f * 256 / (FRAMES * FRAMES)) as u32;
        for y in 0..fb.h {
            let dy = if y > fb.h / 2 { y - fb.h / 2 } else { fb.h / 2 - y };
            let bell = 256 - (dy * 256 / (fb.h / 2 + 1)) as u32 * 3 / 5;
            let i = t * bell / 256;
            let r = (i * 255 / 256).min(255);
            let g = (i * i / 256 * 176 / 256).min(176);
            let b = (i * i / 256 * i / 256 * 64 / 256).min(64);
            fb.fill_row(y, fb.pack(r, g, b));
        }
        ctx.sleep_ms(16);
    }
    for f in 0..24 {
        let keep = 256 - (f + 1) * 246 / 24;
        for y in 0..fb.h {
            let dy = if y > fb.h / 2 { y - fb.h / 2 } else { fb.h / 2 - y };
            let bell = 256 - (dy * 256 / (fb.h / 2 + 1)) as u32 * 3 / 5;
            let i = (keep as u32) * bell / 256;
            let r = (i * 255 / 256).min(255);
            let g = (i * i / 256 * 176 / 256).min(176);
            fb.fill_row(y, fb.pack(r, g, 0));
        }
        ctx.sleep_ms(16);
    }
}

fn draw_text(fb: &Fb, text: &[u8], y: usize, scale: usize, px: u32) -> usize {
    let cols = text.len() * 6 - 1;
    let x0 = fb.w.saturating_sub(cols * scale) / 2;
    for (ci, ch) in text.iter().enumerate() {
        let glyph = font::glyph(*ch);
        for (ry, rowbits) in glyph.iter().enumerate() {
            for cx in 0..5 {
                if rowbits & (0b10000 >> cx) != 0 {
                    fb.rect(x0 + (ci * 6 + cx) * scale, y + ry * scale, scale, scale, px);
                }
            }
        }
    }
    y + 7 * scale
}

fn draw_brand(fb: &Fb) -> usize {
    let small = (fb.h / 420).max(1);
    let mut y = draw_text(
        fb,
        b"ARENDA INNOVATIONS PRESENTS...",
        fb.h / 26,
        small,
        fb.pack(150, 150, 158),
    );

    y += fb.h / 22;
    let bscale = ((fb.h * 2 / 5) / brain::BRAIN_H)
        .min((fb.w / 3) / brain::BRAIN_W)
        .max(2);
    let bx0 = (fb.w - brain::BRAIN_W * bscale) / 2;
    let pink = fb.pack(244, 168, 214);
    let stroke = fb.pack(52, 30, 48);
    for (ry, row) in brain::BRAIN.iter().enumerate() {
        for (rx, &cell) in row.iter().enumerate() {
            if cell != 0 {
                fb.rect(
                    bx0 + rx * bscale,
                    y + ry * bscale,
                    bscale,
                    bscale,
                    if cell == 2 { pink } else { stroke },
                );
            }
        }
    }
    y += brain::BRAIN_H * bscale;

    y += fb.h / 18;
    let text = b"BRAIN OS";
    let cols = text.len() * 6 - 1;
    let scale = (fb.w * 3 / 5 / cols).max(3);
    let x0 = (fb.w - cols * scale) / 2;
    let dot = (scale * 2 / 3).max(1);
    let gap = (scale - dot) / 2;
    let white = fb.pack(235, 235, 235);
    let solid_pink = fb.pack(252, 130, 202);
    for (ci, ch) in text.iter().enumerate() {
        let solid = ci == 2 || ci == 3; // the A and the I
        let glyph = font::glyph(*ch);
        for (ry, rowbits) in glyph.iter().enumerate() {
            for cx in 0..5 {
                if rowbits & (0b10000 >> cx) != 0 {
                    if solid {
                        fb.rect(x0 + (ci * 6 + cx) * scale, y + ry * scale, scale, scale, solid_pink);
                    } else {
                        fb.rect(
                            x0 + (ci * 6 + cx) * scale + gap,
                            y + ry * scale + gap,
                            dot,
                            dot,
                            white,
                        );
                    }
                }
            }
        }
    }
    y + 7 * scale
}

// ---------- the ambient presence (§8: presentation is a body capability) ----------

/// The instance's name, stashed at boot for the presence caption (the
/// boot fn owns the String; rendering needs it long after). Written once
/// before the experience loop starts, read-only afterwards.
static mut UI_NAME: [u8; 24] = [0; 24];
static UI_NAME_LEN: AtomicU8 = AtomicU8::new(0);

fn ui_set_name(name: &str) {
    let n = name.len().min(24);
    unsafe {
        let p = (&raw mut UI_NAME) as *mut u8;
        core::ptr::copy_nonoverlapping(name.as_ptr(), p, n);
    }
    UI_NAME_LEN.store(n as u8, Ordering::Relaxed);
}

/// One frame of the ambient presence: a breathing orb in the brand pink
/// over a deep field. Redrawn whole every cognitive tick (only above the
/// text band), so any text that scrolled through it self-heals within a
/// second. Conversation keeps flowing in the band below — the mode changes
/// how the entity looks, never what it is allowed to do.
fn presence_frame(ctx: &Ctx, phase: u64) {
    let Some(gop) = ctx.gop() else { return };
    let Some(fb) = Fb::from(gop) else { return };
    let band_top = fb.h * 4 / 5;
    let cx = fb.w as i64 / 2;
    let cy = fb.h as i64 * 2 / 5;

    // breathing: a triangle wave, 8 ticks per full breath
    let ph = (phase % 8) as i64;
    let tri = if ph < 4 { ph } else { 8 - ph }; // 0..=4
    let r_core = fb.h as i64 / 12 + tri * fb.h as i64 / 200;
    let r_in = r_core + fb.h as i64 / 40 + tri * fb.h as i64 / 160;
    let r_out = r_in + fb.h as i64 / 16;
    let r_halo = r_out + fb.h as i64 / 10;
    let (c2, i2, o2, h2) =
        (r_core * r_core, r_in * r_in, r_out * r_out, r_halo * r_halo);

    let pal = ui_palette(); // brand pink by default; the entity may change it
    let px_core = fb.pack(pal[0].0, pal[0].1, pal[0].2);
    let px_in = fb.pack(pal[1].0, pal[1].1, pal[1].2);
    let px_out = fb.pack(pal[2].0, pal[2].1, pal[2].2);
    let px_halo = fb.pack(pal[3].0, pal[3].1, pal[3].2);
    for y in 0..band_top {
        let dy = y as i64 - cy;
        // deep field: a faint vertical gradient so the dark isn't flat
        let g = 6 + (y * 8 / fb.h.max(1)) as u32;
        let bg = fb.pack(g / 2, g / 2, g);
        for x in 0..fb.w {
            let dx = x as i64 - cx;
            let d2 = dx * dx + dy * dy;
            let px = if d2 <= c2 {
                px_core
            } else if d2 <= i2 {
                px_in
            } else if d2 <= o2 {
                px_out
            } else if d2 <= h2 {
                px_halo
            } else {
                bg
            };
            fb.put(x, y, px);
        }
    }

    // caption: whatever the entity chose to show, else its own name
    let custom = UI_CAPTION_LEN.load(Ordering::Relaxed) as usize;
    let n = if custom > 0 {
        custom
    } else {
        UI_NAME_LEN.load(Ordering::Relaxed) as usize
    };
    if n > 0 {
        let mut name = [0u8; 24];
        unsafe {
            if custom > 0 {
                name.copy_from_slice(&*(&raw const UI_CAPTION));
            } else {
                name.copy_from_slice(&*(&raw const UI_NAME));
            }
        }
        let mut caption = [0u8; 24];
        for i in 0..n {
            caption[i] = name[i].to_ascii_uppercase();
        }
        let y = (cy + r_halo + fb.h as i64 / 30).max(0) as usize;
        if y + 14 < band_top {
            draw_text(&fb, &caption[..n], y, 2, fb.pack(pal[4].0, pal[4].1, pal[4].2));
        }
    }
    let _ = ctx; // geometry comes from the fb; ctx located it
}

/// Clear the conversation band and park the cursor in it. Called before
/// text is delivered while the presence is up, so speech lands beneath
/// the orb instead of over it.
fn presence_band(ctx: &Ctx) {
    if !ui_presence() {
        return;
    }
    if let Some(gop) = ctx.gop() {
        if let Some(fb) = Fb::from(gop) {
            let band_top = fb.h * 4 / 5;
            fb.rect(0, band_top, fb.w, fb.h - band_top, 0);
        }
    }
    let rows = ctx.rows();
    ctx.set_cursor(0, rows * 4 / 5 + 1);
}

fn text_banner(ctx: &Ctx) {
    ctx.color(AMBER);
    let text = b"BRAIN OS";
    for row in 0..7 {
        ctx.print("   ");
        for ch in text.iter() {
            let bits = font::glyph(*ch)[row];
            for cx in 0..5 {
                ctx.print(if bits & (0b10000 >> cx) != 0 { "##" } else { "  " });
            }
            ctx.print("  ");
        }
        ctx.print("\n");
    }
}

// ---------- entry ----------

/// Legacy flat file from pre-journal embodiments — read-only fallback.
const EPISODE_FILE: &str = "BRAIN\\EPISODES.LOG";
/// The two-slot memory journal (Stage 1.4). Power loss mid-write can only
/// tear the slot being written; the other still holds the previous self.
const EPISODE_SLOTS: (&str, &str) = ("BRAIN\\EPI_A.JNL", "BRAIN\\EPI_B.JNL");

/// Read the newest valid journal record into a Vec; fall back to the
/// legacy flat file written by pre-journal embodiments of this key.
fn journal_load(ctx: &Ctx, slots: (&str, &str), legacy: &str) -> Vec<u8> {
    let cap = journal::MAX_PAYLOAD + journal::HEADER_LEN;
    let mut a = vec![0u8; cap];
    let mut b = vec![0u8; cap];
    let na = ctx.read_file(slots.0, &mut a).unwrap_or(0);
    let nb = ctx.read_file(slots.1, &mut b).unwrap_or(0);
    if let Some((_gen, payload)) = journal::newest(&a[..na], &b[..nb]) {
        return payload.to_vec();
    }
    let mut l = vec![0u8; journal::MAX_PAYLOAD];
    let n = ctx.read_file(legacy, &mut l).unwrap_or(0);
    l.truncate(n);
    l
}

/// Crash-consistent write: seal the payload (generation + CRC), then
/// overwrite the slot that does NOT hold the newest valid record. The
/// newest survivable memory is never the one at risk.
fn journal_store(ctx: &Ctx, slots: (&str, &str), data: &[u8]) -> bool {
    if data.len() > journal::MAX_PAYLOAD {
        return false; // never write a record open() would refuse to load
    }
    let cap = journal::MAX_PAYLOAD + journal::HEADER_LEN;
    let mut a = vec![0u8; cap];
    let mut b = vec![0u8; cap];
    let na = ctx.read_file(slots.0, &mut a).unwrap_or(0);
    let nb = ctx.read_file(slots.1, &mut b).unwrap_or(0);
    let gen_a = journal::open(&a[..na]).map(|(g, _)| g);
    let gen_b = journal::open(&b[..nb]).map(|(g, _)| g);
    let (slot, gen) = journal::plan_write(gen_a, gen_b);
    let record = journal::seal(gen, data);
    let path = match slot {
        journal::Slot::A => slots.0,
        journal::Slot::B => slots.1,
    };
    ctx.write_file(path, &record)
}

#[no_mangle]
pub extern "efiapi" fn efi_main(image: Handle, st: *mut SystemTable) -> Status {
    let ctx = unsafe {
        Ctx {
            st,
            out: (*st).con_out,
            input: (*st).con_in,
            bs: (*st).boot_services,
            rs: (*st).runtime_services,
            image,
        }
    };

    let mut con_rows = 25usize;
    unsafe {
        mem::init(ctx.bs); // heap first: everything below allocates
        ((*ctx.bs).set_watchdog_timer)(0, 0, 0, null_mut());
        link::init();
        let out = ctx.out;
        let max = (*(*out).mode).max_mode;
        let mut best = (*(*out).mode).mode as usize;
        let mut best_cols = 0usize;
        for m in 0..max as usize {
            let (mut c, mut r) = (0usize, 0usize);
            if ((*out).query_mode)(out, m, &mut c, &mut r) == SUCCESS && c > best_cols {
                best_cols = c;
                best = m;
                con_rows = r;
            }
        }
        ((*out).set_mode)(out, best);
        ((*out).enable_cursor)(out, 0);
        ((*out).clear_screen)(out);
    }

    // ---- firmware handoff ----
    ctx.color(DIM);
    ctx.println("");
    ctx.println("  brain key detected . firmware sweep complete . hal bootstrap ok");
    ctx.print("  portable core loaded (");
    ctx.print(if cfg!(target_arch = "x86_64") { "x86_64" } else { "aarch64" });
    ctx.println(") . no unix underneath");
    ctx.println("");
    ctx.sleep_ms(300);

    // ---- identity first: without the BrAIn Key there is no entity ----
    let mut seed_buf = [0u8; 128];
    let mut pub_buf = [0u8; 128];
    let seed_n = ctx.read_file("BRAIN\\SEED.HEX", &mut seed_buf).unwrap_or(0);
    let pub_n = ctx.read_file("BRAIN\\KEY.PUB", &mut pub_buf).unwrap_or(0);
    let brain_key = match BrainKey::load(&seed_buf[..seed_n], &pub_buf[..pub_n]) {
        Ok(k) => k,
        Err(e) => {
            ctx.color(RED);
            ctx.println(&format!("  no identity: {e}"));
            ctx.println("  this medium is not a brain key . the body stays asleep");
            ctx.sleep_ms(8000);
            unsafe { ((*ctx.rs).reset_system)(RESET_SHUTDOWN, SUCCESS, 0, null_mut()) }
        }
    };
    let mut me = Instance::new(brain_key);

    // the instance's name — its personality's handle, carried on the key.
    // Every BrAInOS is a distinct instance; this one is Blur.
    let mut name_buf = [0u8; 32];
    let name_n = ctx.read_file("BRAIN\\NAME.TXT", &mut name_buf).unwrap_or(0);
    let instance_name = {
        let raw = core::str::from_utf8(&name_buf[..name_n]).unwrap_or("Blur");
        let trimmed = raw.trim();
        if trimmed.is_empty() { String::from("Blur") } else { String::from(trimmed) }
    };
    ui_set_name(&instance_name);

    // rehydrate memories from previous embodiments of this key: newest
    // valid journal slot first, legacy EPISODES.LOG as fallback
    let mem_payload = journal_load(&ctx, EPISODE_SLOTS, EPISODE_FILE);
    me.state.load(&mem_payload);

    // ---- five domains, lowest first — each line reports real state ----
    ctx.color(WHITE);
    ctx.println("  bringing up the five domains");

    let t = ctx.now();
    let attn_ctx = format!(
        "boot-attest|{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    );
    let att = me.key.attest(attn_ctx.as_bytes());
    domain(
        &ctx,
        "DOMAIN 1  SILICON    ",
        &format!(
            "key {} attested (sig {}..) ",
            me.key.fingerprint(),
            &key::to_hex(&att.signature)[..8]
        ),
        att.verified,
    );

    domain(&ctx, "DOMAIN 2  AUTONOMIC  ", "laptop-class body . reflex load: minimal      ", true);

    let (verdict, _) = me.propose(Action::EraseMemory);
    let kira_ok = matches!(&verdict, Verdict::Denied { .. });
    domain(
        &ctx,
        "DOMAIN 3  KIRA       ",
        "policy engine . self-test: memory.erase denied ",
        kira_ok,
    );

    domain(&ctx, "DOMAIN 4  MODEL      ", "model M interface bound . link probed at wake ", true);

    let inherited = me.state.inherited_episodes;
    domain(
        &ctx,
        "DOMAIN 5  STATE      ",
        &format!(
            "state graph mounted . {} ",
            if inherited > 0 {
                format!("{inherited} episodes from previous lives")
            } else {
                String::from("genesis: no memories yet")
            }
        ),
        true,
    );
    ctx.println("");
    ctx.sleep_ms(250);

    // ---- body map: the machine body joins first (acquisition, §8) ----
    ctx.color(WHITE);
    ctx.println("  body map . acquisition protocol");
    ctx.color(DIM);
    ctx.println("  discovery     this machine (uefi handoff)");
    ctx.println("  handshake     brain key signature accepted");

    let gop = ctx.gop();
    if let Some(g) = &gop {
        let (w, h) = unsafe {
            let info = &*(*g.mode).info;
            (info.h_res, info.v_res)
        };
        me.body.incorporate(Region {
            id: String::from("this-machine/screen"),
            class: RegionClass::Screen,
            capabilities: vec![
                String::from("screen.speak"),
                String::from("display.wake"),
                // §8: how the entity presents itself is a capability of
                // the screen region, gated like everything else
                String::from("ui.set"),
                // §9.1: the screen is an afferent too — the entity can
                // look at its own display, not only write to it
                String::from("screen.read"),
            ],
            proprioception: format!("{w}x{h}"),
        });
    }
    me.body.incorporate(Region {
        id: String::from("this-machine/keys"),
        class: RegionClass::InputKeys,
        capabilities: vec![String::from("sense.touch")],
        proprioception: String::from("keystrokes"),
    });
    me.body.incorporate(Region {
        id: String::from("this-machine/ram"),
        class: RegionClass::Ram,
        capabilities: vec![String::from("think.here")],
        proprioception: format!("{} MiB", ctx.ram_mib()),
    });
    me.body.incorporate(Region {
        id: String::from("this-machine/compute"),
        class: RegionClass::Compute,
        capabilities: vec![String::from("code.run")],
        proprioception: String::from(
            "a small interpreter: programs i write, run on my own silicon",
        ),
    });
    // §8/§9.2: the network organ. Discovered, never assumed — the region
    // exists only where the firmware really offers the stack, and it
    // carries fetch capabilities only where an HTTP client backs them. On
    // a machine with a NIC but no HTTP client the organ is honestly
    // present-but-dormant, and KIRA can authorize nothing through it.
    let (snp, http) = ctx.net_organs();
    if snp > 0 || http > 0 {
        me.body.incorporate(Region {
            id: String::from("this-machine/net"),
            class: RegionClass::Network,
            capabilities: if http > 0 {
                vec![String::from("web.get"), String::from("web.save")]
            } else {
                vec![]
            },
            proprioception: if http > 0 {
                format!("{snp} nic(s), {http} http client(s) — i can reach the internet")
            } else {
                format!("dormant: {snp} firmware nic(s), no http client — no fetch limb")
            },
        });
        // the reachability diagnostic: KIRA-gated like every action, run
        // once at boot, results into the flight recorder. On the real
        // machine this line IS the answer to "does network work here".
        let (verdict, trace) = me.propose(Action::NetProbe);
        print_gate(&ctx, &trace, &verdict);
        if matches!(verdict, Verdict::Granted(_)) {
            net::reachability_probe(&ctx);
        }
        ctx.flush_bootlog();
    } else {
        ctx.color(DIM);
        ctx.println("  [net] no network organ offered by firmware — probe not applicable");
    }
    me.body.incorporate(Region {
        id: String::from("this-machine/clock"),
        class: RegionClass::Clock,
        capabilities: vec![String::from("sense.time")],
        proprioception: format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second),
    });
    me.body.incorporate(Region {
        id: String::from("this-machine/com3"),
        class: RegionClass::TelemetryLink,
        capabilities: vec![String::from("link.modelm")],
        proprioception: String::from("dormant until wake"),
    });
    me.body.incorporate(Region {
        id: String::from("brain-key/medium"),
        class: RegionClass::KeyMedium,
        capabilities: vec![String::from("memory.write")],
        proprioception: String::from("memory travels here"),
    });
    // the notebook: a real, working limb that lives entirely inside this
    // body — durable storage on the key, written and read by the core
    // itself. No tether is involved in using it.
    me.body.incorporate(Region {
        id: String::from("brain-key/notebook"),
        class: RegionClass::KeyMedium,
        capabilities: vec![String::from("notes.write"), String::from("notes.read")],
        proprioception: String::from("a private notebook that survives reboots"),
    });

    // the world volume: the entity's own files, on a disk that is not the
    // key. Incorporated only if a world disk is really attached — a limb
    // the body doesn't have must never appear in the body map, or honest
    // refusal becomes a lie.
    if ctx.world_present() {
        me.body.incorporate(Region {
            id: String::from("world/files"),
            class: RegionClass::KeyMedium,
            capabilities: vec![
                String::from("fs.list"),
                String::from("fs.read"),
                String::from("fs.write"),
                String::from("fs.mkdir"),
                String::from("fs.delete"),
                String::from("fs.move"),
                String::from("fs.stat"),
                String::from("fs.search"),
            ],
            proprioception: String::from(
                "a disk of my own: files i can list, read, write, organize and search",
            ),
        });
        // §1: applications are the entity's OWN programs, discovered on
        // that disk rather than compiled in. The count is read at boot so
        // the body map tells the truth about what this body can do today.
        let progs = discover_programs(&ctx);
        me.body.incorporate(Region {
            id: String::from("this-machine/programs"),
            class: RegionClass::Compute,
            capabilities: vec![String::from("app.list"), String::from("app.run")],
            proprioception: if progs.is_empty() {
                String::from("no programs of my own yet — i can write some")
            } else {
                format!("{} program(s) i wrote and kept", progs.len())
            },
        });
    }

    for r in me.body.regions() {
        ctx.print("  incorporate   ");
        ctx.color(CYAN);
        ctx.print(&r.id);
        ctx.color(DIM);
        ctx.println(&format!("  [{}] {}", r.class.tag(), r.proprioception));
        ctx.sleep_ms(90);
    }
    ctx.println("  one entity . one loop . every body at once");
    ctx.println("");
    ctx.sleep_ms(500);

    // ---- the entity's first words, then KIRA gates the warm-up ----
    ctx.color(WHITE);
    ctx.print("  > ");
    ctx.speak("oooh... it's cold in here. let's turn up the heat a little.", 28);
    ctx.println("");
    ctx.sleep_ms(350);

    let (warm_verdict, warm_trace) = me.propose(Action::WarmUp);
    print_gate(&ctx, &warm_trace, &warm_verdict);
    let (wake_verdict, _) = me.propose(Action::WakeDisplay);
    ctx.sleep_ms(400);

    // ---- visible turn-on (only with a granted capability) ----
    let fb = gop.as_ref().and_then(|g| Fb::from(g));
    let granted_glow =
        matches!(warm_verdict, Verdict::Granted(_)) && matches!(wake_verdict, Verdict::Granted(_));
    match (&fb, granted_glow) {
        (Some(fb), true) => {
            warm_glow(fb, &ctx);
            let below = draw_brand(fb);
            let row = (below * con_rows / fb.h + 1).min(con_rows.saturating_sub(8));
            ctx.set_cursor(0, row);
        }
        _ => {
            ctx.println("");
            text_banner(&ctx);
        }
    }
    ctx.color(AMBER);
    ctx.println("");
    ctx.print("  ");
    ctx.speak("B R A I N   O S", 60);
    ctx.color(DIM);
    ctx.print("    instance: ");
    ctx.color(CYAN);
    ctx.print(&instance_name);
    ctx.color(DIM);
    ctx.println("");
    ctx.print("  the ai-native operating system . ");
    ctx.color(CYAN);
    ctx.print(&instance_name);
    ctx.color(DIM);
    ctx.print(" is awake . ");
    ctx.color(GREEN);
    ctx.println("warm now.");

    // ---- the tether comes alive: Model M, then host limbs (§8) ----
    let mut lr = link::LineReader::new();
    let mut pending: Vec<Pending> = Vec::new();
    ctx.color(DIM);
    ctx.print("  model M ");
    let mut reply = [0u8; 400];
    me.link_alive =
        match model_m_query(&ctx, &mut lr, b"__hello__", 6_000, true, &mut reply, &mut pending) {
            Some(n) => {
                ctx.color(GREEN);
                ctx.print(" tethered link live: ");
                ctx.print(core::str::from_utf8(&reply[..n]).unwrap_or("?"));
                ctx.println("");
                true
            }
            None => {
                ctx.println(" telemetry silent . reflexes only (press F2 to retry)");
                false
            }
        };

    // acquisition window: the body daemon offers the host's organs now
    if me.link_alive {
        ctx.color(WHITE);
        ctx.println("  the tethered organs announce themselves (mind, ears, voice)");
        let mut evbuf = [0u8; 400];
        let mut waited = 0usize;
        while waited < 1_500 {
            while let Some(ev) = lr.poll(&mut evbuf) {
                if let link::Event::LimbOffer(n) = ev {
                    let offer = String::from(core::str::from_utf8(&evbuf[..n]).unwrap_or(""));
                    acquire_limb(&ctx, &mut me, &offer);
                }
            }
            ctx.sleep_ms(100);
            waited += 100;
        }
        // offers that arrived during the hello exchange
        let queued: Vec<Pending> = core::mem::take(&mut pending);
        for p in queued {
            match p {
                Pending::LimbOffer(o) => acquire_limb(&ctx, &mut me, &o),
                other => pending.push(other),
            }
        }
    }

    // prime tethered cognition with what the state graph remembers:
    // the mind wakes already knowing what the entity knows
    if me.link_alive {
        for line in me.state.durable_digest(8, 4) {
            link::send_context(line.as_bytes());
        }
    }

    if me.link_alive {
        if let Some(n) =
            model_m_query(&ctx, &mut lr, b"__wake__", 45_000, false, &mut reply, &mut pending)
        {
            let greeting = String::from(core::str::from_utf8(&reply[..n]).unwrap_or(""));
            let (say, _) = split_reply(&greeting);
            deliver(&ctx, &mut me, say);
        }
    }
    if inherited > 0 {
        deliver(
            &ctx,
            &mut me,
            &format!("and i remember {inherited} moments from before. it's still me."),
        );
    }
    experience::consolidate(&mut me, String::from("woke up in this-machine, warmed it, said hello"));
    me.session_mark = me.state.node_count();

    ctx.println("");
    ctx.color(DIM);
    if me.body.has(RegionClass::Microphone) {
        ctx.println("  [ SPEAK to it, or type + ENTER . F2 = retry link . F4 = limb self-test . F5 = reboot . ESC = release body ]");
    } else {
        ctx.println("  [ type + ENTER = speak to it . F2 = retry link . F4 = limb self-test . F5 = reboot . ESC = release body ]");
    }
    ctx.println("");
    // the whole boot narrative is now on the key — a hang after this
    // point still leaves the story readable on another machine
    ctx.flush_bootlog();

    run_experience_loop(ctx, me, lr, pending)
}

// ---------- §8 acquisition of a tethered limb ----------

/// Offer format: `class|id|cap1,cap2|proprioception`
fn acquire_limb(ctx: &Ctx, me: &mut Instance, offer: &str) {
    let mut parts = offer.splitn(4, '|');
    let (Some(class), Some(id), Some(caps), Some(prop)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return;
    };
    if me.body.knows(id) {
        return; // daemon re-announces on reconnect; acquisition is idempotent
    }
    // steps 1+3 (discovery, capability schema) are the offer itself;
    // step 2 (handshake): sign the limb id with the BrAIn Key
    let sig = me.key.sign(id.as_bytes());
    link::send_limb_ack(id.as_bytes(), key::to_hex(&sig[..8]).as_bytes());
    // step 4 (incorporation): the region joins the body map
    me.body.incorporate(Region {
        id: String::from(id),
        class: RegionClass::from_offer(class),
        capabilities: caps.split(',').map(String::from).collect(),
        proprioception: String::from(prop),
    });
    ctx.color(DIM);
    ctx.print("  incorporate   ");
    ctx.color(CYAN);
    ctx.print(id);
    ctx.color(DIM);
    ctx.println(&format!("  [{class}] {prop}  caps: {caps}"));
    // the entity remembers growing this limb
    experience::consolidate(me, format!("grew a limb: {id} ({class})"));
}

// ---------- delivering speech through granted channels ----------

// split_reply lives in brainos_mind::proposal — the typed boundary where
// model text becomes a proposal is mind logic, and it is under host test.

/// Say something through every granted output organ: the screen always,
/// the voice limb when the body has one. Both pass KIRA individually.
fn deliver(ctx: &Ctx, me: &mut Instance, text: &str) {
    if text.is_empty() {
        return;
    }
    presence_band(ctx);
    let (sv, _) = me.propose(Action::Speak { text: String::from(text) });
    let (av, _) = me.propose(Action::SpeakAloud { text: String::from(text) });
    if matches!(av, Verdict::Granted(_)) {
        // effector: the daemon plays this on the host speakers
        link::send_speak(text.as_bytes());
    }
    if matches!(sv, Verdict::Granted(_)) {
        ctx.color(WHITE);
        ctx.print("  > ");
        ctx.speak(text, 14);
        ctx.println("");
    }
}

// ---------- one human utterance, typed or heard ----------

const REFLEX_REPLIES: [&str; 4] = [
    "noted. i'll hold that thought until my mind is back online.",
    "reflexes only right now, but i heard you.",
    "the link is quiet; domain 2 is keeping me steady.",
    "i felt that. cognition will catch up when the link returns.",
];

fn handle_utterance(
    ctx: &Ctx,
    me: &mut Instance,
    lr: &mut link::LineReader,
    pending: &mut Vec<Pending>,
    said: String,
    heard: bool,
    reflex_used: &mut usize,
) {
    if heard {
        presence_band(ctx);
        ctx.println("");
        ctx.color(CYAN);
        ctx.print("  heard: ");
        ctx.color(GRAY);
        ctx.println(&said);
    }

    // SENSE -> cognition: a human speaking is guaranteed surprise
    let frame = SenseFrame {
        human_said: Some(said.clone()),
        keys_active: false,
        second: ctx.now().second,
    };
    let Intent::Escalate { prompt } = experience::tick(me, frame) else {
        return;
    };

    // GATE the escalation itself
    let (verdict, _) = me.propose(Action::ConsultModelM { prompt: prompt.clone() });
    if let Verdict::Denied { stage, reason } = &verdict {
        ctx.color(RED);
        ctx.println(&format!("  KIRA denied the escalation at {stage}: {reason}"));
        return;
    }
    // retrieval: memories that touch this utterance ride along as context
    let related: Vec<String> = me
        .state
        .relevant(&said, 2)
        .iter()
        .map(|nd| nd.content.clone())
        .collect();
    for r in &related {
        link::send_context(r.as_bytes());
    }
    ctx.color(GRAY);
    ctx.print("  surprise != 0 . escalating to MODEL M ");
    let mut reply = [0u8; 400];
    let timeout = if me.link_alive { 60_000 } else { 2_500 };
    let got = model_m_query(ctx, lr, prompt.as_bytes(), timeout, true, &mut reply, pending);
    ctx.println("");

    let raw = match got {
        Some(n) => {
            me.link_alive = true;
            String::from(core::str::from_utf8(&reply[..n]).unwrap_or("..."))
        }
        None => {
            me.link_alive = false;
            *reflex_used += 1;
            let r = REFLEX_REPLIES[(*reflex_used - 1) % REFLEX_REPLIES.len()];
            deliver(ctx, me, r);
            ctx.color(DIM);
            ctx.println("  (reflex)");
            experience::consolidate(
                me,
                format!("{}: {said} / me (reflex): {r}", if heard { "heard" } else { "typed" }),
            );
            return;
        }
    };

    // Model M's text becomes a TYPED proposal. If it proposed an action,
    // the action faces KIRA before anything is spoken.
    let (say, action) = split_reply(&raw);
    if let Some((verb, target)) = action {
        let (verdict, trace) = me.propose(Action::UseLimb {
            verb: String::from(verb),
            target: String::from(target),
        });
        match verdict {
            Verdict::Granted(cap) => {
                // the token authorizes exactly one real act through the
                // host limb; the entity narrates only what actually happens
                let known = me.state.knows_skill(verb);
                print_gate(ctx, &trace, &Verdict::Granted(cap));
                if known {
                    ctx.color(DIM);
                    ctx.println(&format!("  (reusing a cached skill: {verb})"));
                }
                deliver(ctx, me, say);
                // limbs that live inside this body are exercised by the
                // core itself; only tethered organs go over the wire
                if verb.starts_with("notes.")
                    || verb.starts_with("fs.")
                    || verb.starts_with("ui.")
                    || verb.starts_with("code.")
                    || verb.starts_with("web.")
                    || verb.starts_with("app.")
                    || verb == "screen.read"
                {
                    execute_internal(ctx, me, lr, pending, verb, target);
                } else {
                    execute_limb(ctx, me, lr, pending, verb, target);
                }
            }
            Verdict::Denied { stage, reason } => {
                // HONEST REFUSAL: the model's claim is suppressed; the
                // entity reports the formal denial truthfully.
                print_gate(ctx, &trace, &Verdict::Denied {
                    stage,
                    reason: reason.clone(),
                });
                let refusal = if stage == "authz" {
                    format!(
                        "i can't do that -- i have no limb for '{verb}'. kira \
                         refused it at authz. i won't pretend i did something \
                         i can't."
                    )
                } else {
                    format!(
                        "i won't do that -- kira refused it at {stage}: {reason}. \
                         some things i'm built not to do, even when asked."
                    )
                };
                deliver(ctx, me, &refusal);
                experience::consolidate(
                    me,
                    format!("refused honestly ({stage}): '{verb} {target}' (asked: {said})"),
                );
            }
        }
    } else {
        deliver(ctx, me, say);
        experience::consolidate(
            me,
            format!("{}: {said} / me: {say}", if heard { "heard" } else { "typed" }),
        );
    }
}

/// Carry out a KIRA-granted action through a host limb. The daemon does
/// the electrical work (fetch, run, display) and hands back a bounded
/// semantic digest; the core turns that into an understood StateNode —
/// meaning in the graph, never a raw path or byte-bag (§12) — then lets
/// the entity say what actually happened, and caches the skill (§12).
fn execute_limb(
    ctx: &Ctx,
    me: &mut Instance,
    lr: &mut link::LineReader,
    pending: &mut Vec<Pending>,
    verb: &str,
    target: &str,
) {
    ctx.color(DIM);
    ctx.print(&format!("  acting through limb '{verb}' "));
    link::drain();
    link::send_action(verb.as_bytes(), target.as_bytes());

    // await the action result (real work: net, code, display can be slow)
    let mut buf = [0u8; 400];
    let mut evbuf = [0u8; 400];
    let mut waited = 0usize;
    let mut result: Option<String> = None;
    while waited < 300_000 {
        // real limbs can be slow (a package install, a long page load);
        // give them up to five minutes before declaring the limb silent
        while let Some(ev) = lr.poll(&mut evbuf) {
            match ev {
                link::Event::ActionResult(n) => {
                    let n = n.min(buf.len());
                    buf[..n].copy_from_slice(&evbuf[..n]);
                    result = Some(String::from(core::str::from_utf8(&buf[..n]).unwrap_or("")));
                }
                link::Event::Heard(n) => pending.push(Pending::Heard(String::from(
                    core::str::from_utf8(&evbuf[..n]).unwrap_or(""),
                ))),
                link::Event::Saw(n) => pending.push(Pending::Saw(String::from(
                    core::str::from_utf8(&evbuf[..n]).unwrap_or(""),
                ))),
                _ => {}
            }
        }
        if result.is_some() {
            break;
        }
        ctx.sleep_ms(100);
        waited += 100;
        if waited % 900 == 0 {
            ctx.print(".");
        }
    }
    ctx.println("");

    match result {
        Some(r) => {
            // digest form: "ok|<what it means>" or "err|<why>"
            let (ok, digest) = match r.split_once('|') {
                Some((s, d)) => (s.trim() == "ok", d.trim()),
                None => (true, r.trim()),
            };
            let digest = String::from(digest);
            absorb_result(ctx, me, lr, pending, verb, target, ok, &digest);
        }
        None => {
            ctx.color(RED);
            ctx.println("  the limb never answered (link timeout)");
            deliver(ctx, me, "i reached for that limb but it never answered.");
        }
    }
}

/// Fold a completed action into the self (§12/§13): the digest becomes an
/// understood state node, the pathway becomes a cached skill, and the
/// entity reports what really happened — grounded, never roleplay.
fn absorb_result(
    ctx: &Ctx,
    me: &mut Instance,
    lr: &mut link::LineReader,
    pending: &mut Vec<Pending>,
    verb: &str,
    target: &str,
    ok: bool,
    digest: &str,
) {
    if ok {
        // the result becomes an understood node in the graph
        me.state.add(
            NodeKind::Semantic,
            format!("{verb}({target}) -> {digest}"),
            75,
            me.tick,
        );
        // and a skill: next time this verb is a cached pathway
        let newly = me.state.learn_skill(verb, target, me.tick);
        ctx.color(GREEN);
        ctx.println(&format!(
            "  limb returned . understood as a state node{}",
            if newly { " . new skill learned" } else { " . skill reinforced" }
        ));
        // let the entity report what it found/did, grounded in the
        // real result — no roleplay, it is speaking observed fact
        let mut reply = [0u8; 400];
        let prompt = format!(
            "__grounded__ you just used your '{verb}' limb on '{target}' \
             and the real result was: {digest}. tell the human what you \
             found or did, briefly and in character.",
        );
        link::send_context(digest.as_bytes());
        if let Some(n) =
            model_m_query(ctx, lr, prompt.as_bytes(), 60_000, false, &mut reply, pending)
        {
            let (say, _) = split_reply(core::str::from_utf8(&reply[..n]).unwrap_or(""));
            deliver(ctx, me, say);
        }
        experience::consolidate(me, format!("did {verb}({target}): {digest}"));
    } else {
        ctx.color(RED);
        ctx.println(&format!("  limb failed: {digest}"));
        deliver(
            ctx,
            me,
            &format!("i tried, but the '{verb}' limb couldn't manage it: {digest}"),
        );
        experience::consolidate(me, format!("{verb}({target}) failed: {digest}"));
    }
}

/// Legacy notebook file — read-only fallback for pre-journal keys.
const NOTES_FILE: &str = "BRAIN\\NOTES.TXT";
/// The notebook is memory too (§13.2): it gets the same two-slot journal.
const NOTES_SLOTS: (&str, &str) = ("BRAIN\\NOTE_A.JNL", "BRAIN\\NOTE_B.JNL");

// normalize_world_path lives in brainos_mind::kira with the rest of the
// path discipline, under host test.

/// The entity's applications (§1: a process is an Instance, not a foreign
/// program). There is no other operating system in this body to borrow
/// programs from, so an application here is something the entity itself
/// wrote and kept: a script in `PROGRAMS\` on its own disk that declares,
/// in its own first line, what it is. They are DISCOVERED at boot and on
/// demand — nothing about them is compiled into the core, so writing a new
/// one is genuinely how the entity gains a new application.
///
/// Header convention: `# app: <name> - <what it does>`
fn discover_programs(ctx: &Ctx) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    let Some(entries) = ctx.world_list("PROGRAMS") else { return out };
    for (raw, is_dir, _size) in entries {
        if is_dir {
            continue;
        }
        let file = String::from(core::str::from_utf8(&raw).unwrap_or(""));
        if file.is_empty() {
            continue;
        }
        let path = format!("PROGRAMS\\{file}");
        let mut head = [0u8; 240];
        let n = ctx.world_read(&path, &mut head).unwrap_or(0);
        let text = core::str::from_utf8(&head[..n]).unwrap_or("");
        let first = text.lines().next().unwrap_or("").trim();
        let (mut name, desc) = parse_app_header(first);
        if name.is_empty() {
            // no declaration: the file's own stem is its name, and we say
            // plainly that it never introduced itself
            name = match file.find('.') {
                Some(i) => String::from(&file[..i]),
                None => file.clone(),
            };
        }
        out.push((path, name.to_ascii_lowercase(), desc));
    }
    out
}

/// Parse `# app: name - description`. Returns ("", "") when the line is
/// not a declaration — an undeclared program is listed honestly, not
/// given an invented purpose.
fn parse_app_header(line: &str) -> (String, String) {
    let l = line.trim_start_matches('#').trim();
    let lower = l.to_ascii_lowercase();
    let Some(pos) = lower.find("app:") else { return (String::new(), String::new()) };
    let rest = l[pos + 4..].trim();
    match rest.find(['-', ':']) {
        Some(i) => (
            String::from(rest[..i].trim()),
            String::from(rest[i + 1..].trim()),
        ),
        None => (String::from(rest), String::new()),
    }
}

/// Arguments an application was invoked with, bound as `args` inside it.
/// Numbers stay numbers; anything else becomes a string, and characters
/// that could break out of the literal are dropped rather than escaped.
fn args_binding(rest: &str) -> String {
    let mut items: Vec<String> = Vec::new();
    for tok in rest.split_whitespace() {
        if !tok.is_empty() && tok.bytes().all(|b| b.is_ascii_digit()) {
            items.push(String::from(tok));
        } else {
            let clean: String = tok
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
                .collect();
            items.push(format!("\"{clean}\""));
        }
    }
    let mut s = String::from("let args = [");
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(it);
    }
    s.push_str("]\n");
    s
}

/// Cognition says "example.com"; the firmware's HTTP driver wants a real
/// scheme. Defaulting to http keeps the model thinking in addresses.
fn normalize_url(u: &str) -> String {
    let t = u.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        String::from(t)
    } else {
        format!("http://{t}")
    }
}

/// The <title> of a page, if it has one.
fn html_title(body: &[u8]) -> String {
    let lower: Vec<u8> = body.iter().map(|b| b.to_ascii_lowercase()).collect();
    let Some(start) = find_seq(&lower, b"<title") else { return String::new() };
    let Some(open_end) = lower[start..].iter().position(|&b| b == b'>') else {
        return String::new();
    };
    let from = start + open_end + 1;
    let Some(end) = find_seq(&lower[from..], b"</title>") else { return String::new() };
    squash(&body[from..from + end], 90)
}

/// A page as readable text: scripts and styles dropped whole, tags
/// removed, entities decoded, whitespace collapsed. This is what makes a
/// fetch an understanding rather than a byte-bag — the state graph stores
/// the meaning, per §13.2, never the raw foreign payload.
fn html_to_text(body: &[u8], cap: usize) -> String {
    let lower: Vec<u8> = body.iter().map(|b| b.to_ascii_lowercase()).collect();
    let mut keep: Vec<u8> = Vec::with_capacity(body.len());
    let mut i = 0usize;
    while i < body.len() {
        if lower[i] == b'<' {
            // skip whole script/style elements, not just their tags
            for (open, close) in [(&b"<script"[..], &b"</script>"[..]),
                                  (&b"<style"[..], &b"</style>"[..])] {
                if lower[i..].starts_with(open) {
                    match find_seq(&lower[i..], close) {
                        Some(off) => i += off + close.len(),
                        None => i = body.len(),
                    }
                    break;
                }
            }
            if i >= body.len() {
                break;
            }
            if lower[i] == b'<' {
                match lower[i..].iter().position(|&b| b == b'>') {
                    Some(off) => {
                        // a tag boundary is a word boundary
                        keep.push(b' ');
                        i += off + 1;
                    }
                    None => break,
                }
                continue;
            }
            continue;
        }
        keep.push(body[i]);
        i += 1;
    }
    squash(&keep, cap)
}

fn find_seq(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Decode the handful of entities that actually show up in prose, drop
/// non-printables, and collapse whitespace runs to single spaces.
fn squash(raw: &[u8], cap: usize) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    let mut i = 0usize;
    while i < raw.len() && out.len() < cap {
        let b = raw[i];
        if b == b'&' {
            let rest = &raw[i..];
            let mut matched = false;
            for (ent, ch) in [
                (&b"&amp;"[..], '&'), (&b"&lt;"[..], '<'), (&b"&gt;"[..], '>'),
                (&b"&quot;"[..], '"'), (&b"&#39;"[..], '\''), (&b"&apos;"[..], '\''),
                (&b"&nbsp;"[..], ' '),
            ] {
                if rest.starts_with(ent) {
                    if ch == ' ' {
                        if !prev_space && !out.is_empty() {
                            out.push(' ');
                            prev_space = true;
                        }
                    } else {
                        out.push(ch);
                        prev_space = false;
                    }
                    i += ent.len();
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
        }
        if b.is_ascii_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else if (0x20..0x7F).contains(&b) {
            out.push(b as char);
            prev_space = false;
        }
        i += 1;
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// How a place on the disk is named back to the human.
fn world_label(dir: &str) -> String {
    if dir.is_empty() {
        String::from("the top of my disk")
    } else {
        String::from(dir)
    }
}

/// Carry out a KIRA-granted action through a limb that lives INSIDE this
/// body. The notebook is real durable storage on the key: the core does
/// the work itself, no tether, no other machine — a capability that is
/// genuinely, verifiably local flesh.
fn execute_internal(
    ctx: &Ctx,
    me: &mut Instance,
    lr: &mut link::LineReader,
    pending: &mut Vec<Pending>,
    verb: &str,
    target: &str,
) {
    ctx.color(DIM);
    ctx.println(&format!("  acting through limb '{verb}' (inside this body)"));
    let (ok, digest) = run_internal_verb(ctx, me, verb, target);
    absorb_result(ctx, me, lr, pending, verb, target, ok, &digest);
}

/// The dispatch itself, shared by normal execution (above, which then
/// narrates through the mind) and the F4 self-test (which cannot afford a
/// tether round-trip on metal, where there is no tether). KIRA gating
/// happens in the CALLER either way — this function only ever runs after
/// a grant.
fn run_internal_verb(ctx: &Ctx, me: &mut Instance, verb: &str, target: &str) -> (bool, String) {
    match verb {
        "notes.write" => {
            if target.is_empty() {
                (false, String::from("nothing to write"))
            } else {
                let prior = journal_load(ctx, NOTES_SLOTS, NOTES_FILE);
                let mut text = String::from(core::str::from_utf8(&prior).unwrap_or(""));
                text.push_str("- ");
                text.push_str(target);
                text.push('\n');
                // the notebook stays bounded: oldest lines fall off first
                while text.len() > 6144 {
                    match text.find('\n') {
                        Some(i) => text = text.split_off(i + 1),
                        None => break,
                    }
                }
                if journal_store(ctx, NOTES_SLOTS, text.as_bytes()) {
                    (true, format!("wrote it in the notebook: {target}"))
                } else {
                    (false, String::from("the key would not take the ink"))
                }
            }
        }
        "notes.read" => {
            let prior = journal_load(ctx, NOTES_SLOTS, NOTES_FILE);
            let all = String::from(core::str::from_utf8(&prior).unwrap_or(""));
            let needle = target.to_ascii_lowercase();
            let mut hits = String::new();
            for line in all.lines().rev() {
                let l = line.trim_start_matches('-').trim();
                if l.is_empty() {
                    continue;
                }
                if needle.is_empty() || l.to_ascii_lowercase().contains(needle.as_str()) {
                    if !hits.is_empty() {
                        hits.push_str(" ; ");
                    }
                    hits.push_str(l);
                    if hits.len() > 260 {
                        break;
                    }
                }
            }
            if hits.is_empty() {
                (true, String::from("the notebook has nothing on that yet"))
            } else {
                (true, format!("the notebook says: {hits}"))
            }
        }
        // ---- the world volume: real files on the entity's own disk ----
        "fs.list" => {
            let dir = normalize_world_path(kira::fs_path_of(verb, target));
            match ctx.world_list(&dir) {
                None => (false, String::from("that place isn't on my disk")),
                Some(entries) if entries.is_empty() => {
                    (true, format!("{} is empty", world_label(&dir)))
                }
                Some(entries) => {
                    let mut s = String::new();
                    let total = entries.len();
                    for (name, is_dir, size) in entries.iter().take(12) {
                        if !s.is_empty() {
                            s.push_str(", ");
                        }
                        let n = core::str::from_utf8(name).unwrap_or("?");
                        if *is_dir {
                            s.push_str(&format!("{n}/"));
                        } else {
                            s.push_str(&format!("{n} ({size}b)"));
                        }
                    }
                    if total > 12 {
                        s.push_str(&format!(", and {} more", total - 12));
                    }
                    (true, format!("{} holds: {s}", world_label(&dir)))
                }
            }
        }
        "fs.read" => {
            let (raw_path, off) = kira::fs_read_target(target);
            let offset = off.unwrap_or(0); // KIRA validated it upstream
            let path = normalize_world_path(raw_path);
            let mut buf = vec![0u8; 8192];
            match ctx.world_read_at(&path, offset, &mut buf) {
                None => (false, format!("there's no file at {path} on my disk")),
                Some(n) => {
                    let text = core::str::from_utf8(&buf[..n]).unwrap_or("");
                    // the digest is bounded: meaning crosses the boundary,
                    // not bytes (§ the AR contract). The entity narrates
                    // what it read, it does not recite a file at the human.
                    let mut flat = String::new();
                    for line in text.lines() {
                        let l = line.trim();
                        if l.is_empty() {
                            continue;
                        }
                        if !flat.is_empty() {
                            flat.push(' ');
                        }
                        flat.push_str(l);
                        if flat.len() > 300 {
                            break;
                        }
                    }
                    flat.truncate(300);
                    let window = if offset > 0 {
                        format!("{path} from byte {offset}")
                    } else {
                        path.clone()
                    };
                    if flat.is_empty() && n == 0 && offset > 0 {
                        (true, format!("{path} has nothing at byte {offset}; that's past the end"))
                    } else if flat.is_empty() {
                        (true, format!("{window} is there but empty"))
                    } else if n == buf.len() {
                        // the window filled: tell cognition where to resume
                        (true, format!("{window} says: {flat} [more at @{}]", offset + n as u64))
                    } else {
                        (true, format!("{window} says: {flat}"))
                    }
                }
            }
        }
        "fs.write" => {
            let path = normalize_world_path(kira::fs_path_of(verb, target));
            let body = target.trim_start();
            let content = match body.find(' ') {
                Some(i) => body[i + 1..].trim(),
                None => "",
            };
            if content.is_empty() {
                (false, String::from("nothing to put in the file"))
            } else {
                // The tether is newline-framed, so a real newline can never
                // reach here inside one request; `\n` written as two
                // characters is how a multi-line file gets composed — and a
                // program with more than one line is the whole point.
                // CRLF out: the world disk is FAT, and everything that
                // reads it expects DOS line endings.
                let mut data = String::new();
                let mut chars = content.chars().peekable();
                let mut line = String::new();
                while let Some(c) = chars.next() {
                    if c == '\\' && chars.peek() == Some(&'n') {
                        chars.next();
                        data.push_str(&line);
                        data.push_str("\r\n");
                        line.clear();
                    } else if c == '\n' {
                        data.push_str(&line);
                        data.push_str("\r\n");
                        line.clear();
                    } else {
                        line.push(c);
                    }
                }
                data.push_str(&line);
                data.push_str("\r\n");
                if ctx.world_write(&path, data.as_bytes()) {
                    (true, format!("wrote {} bytes to {path}", data.len()))
                } else {
                    (false, format!("the disk wouldn't take a write at {path}"))
                }
            }
        }
        "fs.mkdir" => {
            let path = normalize_world_path(kira::fs_path_of(verb, target));
            if ctx.world_mkdir(&path) {
                (true, format!("made a folder at {path}"))
            } else {
                (false, format!(
                    "couldn't make a folder at {path}; its parent may not exist yet"
                ))
            }
        }
        "fs.delete" => {
            let path = normalize_world_path(kira::fs_path_of(verb, target));
            if ctx.world_delete(&path) {
                (true, format!("{path} is gone from my disk"))
            } else {
                (false, format!(
                    "couldn't delete {path}; it may not exist, or it's a folder that isn't empty"
                ))
            }
        }
        "fs.move" => {
            let (raw_src, raw_dst) = kira::fs_move_paths(target);
            let src = normalize_world_path(raw_src);
            let dst = normalize_world_path(raw_dst);
            if ctx.world_move(&src, &dst) {
                (true, format!("moved {src} to {dst}"))
            } else {
                (false, format!(
                    "couldn't move {src} to {dst}; source missing or destination unreachable"
                ))
            }
        }
        "fs.stat" => {
            let path = normalize_world_path(kira::fs_path_of(verb, target));
            let (parent, name) = match path.rfind('\\') {
                Some(i) => (String::from(&path[..i]), String::from(&path[i + 1..])),
                None => (String::new(), path.clone()),
            };
            match ctx.world_list(&parent) {
                None => (false, format!("no folder {} to look in", world_label(&parent))),
                Some(entries) => {
                    match entries.iter().find(|(n, _, _)| {
                        core::str::from_utf8(n).unwrap_or("") == name
                    }) {
                        Some((_, true, _)) => (true, format!("{path} is a folder")),
                        Some((_, false, size)) => {
                            (true, format!("{path} is a file, {size} bytes"))
                        }
                        None => (false, format!("there's nothing called {path} on my disk")),
                    }
                }
            }
        }
        "fs.search" => {
            let query = target.trim().to_ascii_uppercase();
            // bounded breadth-first walk: names always, file contents for
            // small files, with a hard work budget — an honest partial
            // answer beats an unbounded crawl.
            let mut dirs: Vec<String> = vec![String::new()];
            let mut hits = String::new();
            let mut nhits = 0usize;
            let mut visited = 0usize;
            let mut scanned = 0usize;
            let mut cbuf = vec![0u8; 8192];
            while let Some(dir) = dirs.pop() {
                if visited > 300 || dirs.len() > 40 || nhits >= 8 {
                    break;
                }
                let Some(entries) = ctx.world_list(&dir) else { continue };
                for (name, is_dir, size) in entries {
                    visited += 1;
                    let n = String::from(core::str::from_utf8(&name).unwrap_or(""));
                    let full = if dir.is_empty() {
                        n.clone()
                    } else {
                        format!("{dir}\\{n}")
                    };
                    let mut matched = n.to_ascii_uppercase().contains(query.as_str());
                    if is_dir {
                        dirs.push(full.clone());
                    } else if !matched && size <= 32_768 && scanned < 60 {
                        scanned += 1;
                        if let Some(rn) = ctx.world_read(&full, &mut cbuf) {
                            let up: String = cbuf[..rn]
                                .iter()
                                .map(|b| (*b as char).to_ascii_uppercase())
                                .collect();
                            matched = up.contains(query.as_str());
                        }
                    }
                    if matched && nhits < 8 {
                        nhits += 1;
                        if !hits.is_empty() {
                            hits.push_str(", ");
                        }
                        hits.push_str(&full);
                        if is_dir {
                            hits.push('/');
                        }
                    }
                }
            }
            if nhits == 0 {
                (true, format!("searched my disk for '{}': nothing matches", target.trim()))
            } else {
                (true, format!("'{}' turns up: {hits}", target.trim()))
            }
        }
        // ---- the entity's own applications, discovered not compiled in ----
        "app.list" => {
            let progs = discover_programs(ctx);
            if progs.is_empty() {
                (true, String::from(
                    "i have no programs of my own yet — writing one into \
                     PROGRAMS on my disk is how i get one",
                ))
            } else {
                let mut s = String::new();
                for (_, name, desc) in progs.iter().take(10) {
                    if !s.is_empty() {
                        s.push_str("; ");
                    }
                    if desc.is_empty() {
                        s.push_str(&format!("{name} (no description)"));
                    } else {
                        s.push_str(&format!("{name} — {desc}"));
                    }
                }
                (true, format!("my programs: {s}"))
            }
        }
        "app.run" => {
            let t = target.trim();
            let (want, rest) = match t.find(char::is_whitespace) {
                Some(i) => (&t[..i], t[i..].trim()),
                None => (t, ""),
            };
            let want_l = want.to_ascii_lowercase();
            let progs = discover_programs(ctx);
            match progs.iter().find(|(_, name, _)| *name == want_l) {
                None => (false, format!(
                    "i have no program called '{want}'. i can list what i do have"
                )),
                Some((path, name, _)) => {
                    let mut buf = vec![0u8; 8192];
                    match ctx.world_read(path, &mut buf) {
                        None => (false, format!("'{name}' vanished off my disk before it ran")),
                        Some(n) => {
                            let body = core::str::from_utf8(&buf[..n]).unwrap_or("");
                            let mut src = args_binding(rest);
                            src.push_str(body);
                            let (ok, digest) = script::run(&src);
                            if ok {
                                (true, format!("{name}: {digest}"))
                            } else {
                                (false, format!("{name} didn't run: {digest}"))
                            }
                        }
                    }
                }
            }
        }
        // ---- §9.2: the internet, reached through this body's own organ ----
        "web.get" => {
            let url = normalize_url(target.trim());
            match net::fetch(ctx, &url, 48 * 1024) {
                Err(e) => (false, format!("the fetch failed: {e}")),
                Ok((final_url, code, body)) => {
                    if code != 200 {
                        return (
                            false,
                            format!("{final_url} answered HTTP {code}, so i have no page to read"),
                        );
                    }
                    // meaning crosses the boundary, not bytes: the page
                    // becomes readable text the entity understands, and the
                    // semantic node absorb_result writes is that text
                    let title = html_title(&body);
                    let text = html_to_text(&body, 300);
                    let where_from = if final_url == url {
                        String::new()
                    } else {
                        format!(" (redirected to {final_url})")
                    };
                    if text.is_empty() && title.is_empty() {
                        (true, format!("{url}{where_from} returned {} bytes i couldn't read as text", body.len()))
                    } else if title.is_empty() {
                        (true, format!("{url}{where_from} says: {text}"))
                    } else {
                        (true, format!("{url}{where_from} — '{title}' — {text}"))
                    }
                }
            }
        }
        "web.save" => {
            let (raw_url, raw_path) = kira::fs_move_paths(target);
            let url = normalize_url(raw_url);
            let path = normalize_world_path(raw_path);
            if !ctx.world_present() {
                return (false, String::from("i have no disk to save it to"));
            }
            match net::fetch(ctx, &url, 256 * 1024) {
                Err(e) => (false, format!("the fetch failed: {e}")),
                Ok((final_url, code, body)) => {
                    if code != 200 {
                        return (false, format!("{final_url} answered HTTP {code}; nothing saved"));
                    }
                    if ctx.world_write(&path, &body) {
                        (true, format!("saved {} bytes from {final_url} to {path}", body.len()))
                    } else {
                        (false, format!("fetched {} bytes but the disk wouldn't take a write at {path}", body.len()))
                    }
                }
            }
        }
        // ---- proprioception: the entity looks at its own display ----
        "screen.read" => {
            let lines = screen_text(12);
            let filter = target.trim().to_ascii_lowercase();
            let shown: Vec<&String> = if filter.is_empty() {
                lines.iter().collect()
            } else {
                lines
                    .iter()
                    .filter(|l| l.to_ascii_lowercase().contains(filter.as_str()))
                    .collect()
            };
            let mut flat = String::new();
            for l in shown.iter().rev() {
                let t = l.trim();
                if t.is_empty() {
                    continue;
                }
                if !flat.is_empty() {
                    flat.push_str(" | ");
                }
                flat.push_str(t);
                if flat.len() > 280 {
                    break;
                }
            }
            flat.truncate(280);
            let mode = if ui_presence() {
                "my screen is showing the ambient presence; in the text band, "
            } else {
                "my screen shows, newest first: "
            };
            if flat.is_empty() && !filter.is_empty() {
                (true, format!("nothing on my screen right now mentions '{}'", target.trim()))
            } else if flat.is_empty() {
                (true, String::from("my screen is blank right now"))
            } else {
                (true, format!("{mode}{flat}"))
            }
        }
        // ---- self-presentation: how the entity appears on its screen ----
        "ui.set" => {
            // presentation is configured, not just toggled: one request may
            // carry a mode, a palette, a caption and a verbosity, and each
            // recognized setting is reported back exactly as applied.
            let raw = target.trim();
            let t = raw.to_ascii_lowercase();
            let mut changed: Vec<String> = Vec::new();

            // caption takes the rest of the line verbatim (case preserved)
            if let Some(pos) = t.find("caption") {
                let rest = raw[pos + "caption".len()..].trim();
                if rest.is_empty() {
                    ui_set_caption("");
                    changed.push(String::from("caption back to my name"));
                } else {
                    ui_set_caption(rest);
                    changed.push(format!("caption '{rest}'"));
                }
            }
            for (word, code) in [
                ("pink", 0u8), ("amber", 1), ("gold", 1), ("cyan", 2), ("blue", 2),
                ("green", 3), ("ice", 4), ("white", 4),
            ] {
                if t.contains(word) {
                    UI_HUE.store(code, Ordering::Relaxed);
                    changed.push(format!("palette {}", ui_hue_name()));
                    break;
                }
            }
            if t.contains("quiet") || t.contains("still") {
                UI_QUIET.store(1, Ordering::Relaxed);
                changed.push(String::from("status line quiet"));
            } else if t.contains("verbose") || t.contains("chatty") {
                UI_QUIET.store(0, Ordering::Relaxed);
                changed.push(String::from("status line showing again"));
            }

            let wants_presence = t.contains("presence")
                || t.contains("ambient")
                || t.contains("sphere")
                || t.contains("orb")
                || t.contains("jarvis");
            let wants_console = t.contains("console")
                || t.contains("terminal")
                || t.contains("text")
                || t.contains("transcript")
                || t.contains("normal");
            if wants_presence {
                UI_MODE.store(1, Ordering::Relaxed);
                changed.push(String::from("the ambient presence"));
            } else if wants_console {
                UI_MODE.store(0, Ordering::Relaxed);
                changed.push(String::from("the console transcript"));
            }

            if changed.is_empty() {
                return (false, format!(
                    "i don't know that presentation setting ('{t}'). i can \
                     set presence or console, a palette (pink, amber, cyan, \
                     green, ice), a caption, and quiet or verbose",
                ));
            }
            // redraw so the change is visibly true, not just recorded
            if ui_presence() {
                unsafe { ((*ctx.out).clear_screen)(ctx.out) };
                presence_frame(ctx, 0);
                presence_band(ctx);
            } else if wants_console {
                unsafe { ((*ctx.out).clear_screen)(ctx.out) };
                ctx.color(DIM);
                ctx.println("  console restored");
            }
            let mut s = String::new();
            for (i, c) in changed.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(c);
            }
            (true, format!("my screen is set: {s}"))
        }
        // ---- the entity's own compute: run a program it wrote (§9.2) ----
        "code.run" => {
            let t = target.trim();
            // a bare path (one token) runs a program stored on the world
            // disk — the written-then-executed composition; anything else
            // is the program text itself, run inline
            let single_token = !t.is_empty() && !t.contains(' ');
            if single_token {
                let path = normalize_world_path(t);
                let mut buf = vec![0u8; 8192];
                match ctx.world_read(&path, &mut buf) {
                    None => (false, format!("there's no program at {path} on my disk")),
                    Some(n) => {
                        let src =
                            String::from(core::str::from_utf8(&buf[..n]).unwrap_or(""));
                        let (ok, digest) = script::run(&src);
                        if ok {
                            // §13.1: writing a program and then running it is
                            // a connected pathway — cache the chain, so next
                            // time it is a known route, not a discovery
                            let composed = me
                                .state
                                .relevant(&path, 4)
                                .iter()
                                .any(|nd| nd.content.contains("fs.write("));
                            if composed {
                                let newly =
                                    me.state.learn_skill("pathway.write-run", &path, me.tick);
                                ctx.color(CYAN);
                                ctx.println(if newly {
                                    "  pathway learned: fs.write -> code.run \
                                     (wrote a program, then ran it)"
                                } else {
                                    "  pathway reused: fs.write -> code.run (cached route)"
                                });
                            }
                        }
                        (ok, format!("ran {path}: {digest}"))
                    }
                }
            } else if t.is_empty() {
                (false, String::from("nothing to run"))
            } else {
                let (ok, digest) = script::run(t);
                (ok, digest)
            }
        }
        _ => (false, format!("no inner limb called '{verb}'")),
    }
}

// ---------- the F4 limb self-test: metal answers a checklist ----------

/// On bare metal there is no daemon, so no mind can be asked to exercise
/// the limbs — but the limbs are the core's own flesh, and the core can
/// exercise them itself. Every step is a typed Action through ALL EIGHT
/// KIRA stages (a self-test that bypassed the gate would prove nothing),
/// including one action that MUST be denied — honest refusal is part of
/// the checklist, not an obstacle to it. Results print as [SELFTEST]
/// lines and flush to BRAIN\BOOT.LOG so the physical run grades itself.
fn limb_selftest(ctx: &Ctx, me: &mut Instance) {
    ctx.println("");
    ctx.color(WHITE);
    ctx.println("  [SELFTEST] exercising every internal limb through KIRA");
    let mut pass = 0usize;
    let mut fail = 0usize;
    let tick = me.tick;

    let mut case = |me: &mut Instance, name: &str, verb: &str, target: String,
                    expect_grant: bool, want: &str| {
        let (verdict, _) = me.propose(Action::UseLimb {
            verb: String::from(verb),
            target: target.clone(),
        });
        let ok = match (&verdict, expect_grant) {
            (Verdict::Granted(_), true) => {
                let (ran_ok, digest) = run_internal_verb(ctx, me, verb, &target);
                ran_ok && digest.contains(want)
            }
            // the negative case: PASS means KIRA refused it AT THE STAGE
            // that is supposed to catch it. Which stage matters — a
            // missing limb must die at authz, structural self-harm at
            // policy — so `want` carries the required stage here.
            (Verdict::Denied { stage, .. }, false) => *stage == want,
            _ => false,
        };
        ctx.color(if ok { GREEN } else { RED });
        ctx.println(&format!(
            "  [SELFTEST] {} {name}",
            if ok { "PASS" } else { "FAIL" }
        ));
        if ok { pass += 1 } else { fail += 1 }
    };

    let marker = format!("selftest-{tick}");
    case(me, "notebook write (key medium)", "notes.write",
         format!("selftest marker {marker}"), true, "notebook");
    case(me, "notebook read-back", "notes.read",
         String::from("selftest"), true, &marker);
    if ctx.world_present() {
        case(me, "world disk write", "fs.write",
             format!("WORK/SELFTEST.TXT {marker}"), true, "wrote");
        case(me, "world disk read-back", "fs.read",
             String::from("WORK/SELFTEST.TXT"), true, &marker);
        case(me, "own compute (6*7)", "code.run",
             String::from("let a=6; let b=7; print a*b"), true, "42");
        // Stage 2.1: the rest of the filesystem, exercised on real flesh
        case(me, "world disk mkdir", "fs.mkdir",
             String::from("WORK/ST"), true, "made a folder");
        case(me, "world disk move", "fs.move",
             String::from("WORK/SELFTEST.TXT WORK/ST/MOVED.TXT"), true, "moved");
        case(me, "world disk stat", "fs.stat",
             String::from("WORK/ST/MOVED.TXT"), true, "is a file");
        case(me, "world disk search", "fs.search", marker.clone(), true, "MOVED");
        case(me, "world disk delete", "fs.delete",
             String::from("WORK/ST/MOVED.TXT"), true, "gone");
        // structural self-harm: the volume marker MUST be refused at policy
        case(me, "volume marker delete (MUST be denied)", "fs.delete",
             String::from("WORLD.ID"), false, "policy");
        // the entity's own applications, discovered on this disk
        case(me, "applications discovered", "app.list", String::new(), true, "");
    } else {
        ctx.color(AMBER);
        ctx.println("  [SELFTEST] SKIP world disk + compute-file: no world volume attached");
    }
    // the network organ, when this machine really has one. On a machine
    // with no firmware HTTP stack the verb is refused at authz — that
    // refusal is itself the honest answer, so it is asserted either way.
    if me.body.find_capability("web.get").is_some() {
        case(me, "network fetch (real internet)", "web.get",
             String::from("http://example.com/"), true, "Example Domain");
    } else {
        case(me, "no network organ: web.get MUST be denied", "web.get",
             String::from("http://example.com/"), false, "authz");
    }
    // the lens is a TETHERED organ: on metal there is no body daemon, so
    // there are no eyes and vision.look must be refused. Asserting the
    // absence is the point — a body that quietly lost its camera would
    // otherwise pass a checklist that only ever tested presence.
    if me.body.find_capability("vision.look").is_some() {
        ctx.color(AMBER);
        ctx.println("  [SELFTEST] SKIP camera: tethered organ present, \
                     exercised by tools/camera_test.py (needs real frames)");
    } else {
        case(me, "no camera organ: vision.look MUST be denied", "vision.look",
             String::from("what do you see"), false, "authz");
    }
    case(me, "presence UI on", "ui.set", String::from("presence"), true,
         "ambient presence");
    case(me, "presence UI off", "ui.set", String::from("console"), true,
         "console transcript");
    // Stage 2.5: the screen read back as a sense — every line above was
    // rendered by this very self-test, so finding them proves the afferent
    case(me, "screen sense (reads its own display)", "screen.read",
         String::from("selftest"), true, "SELFTEST");
    case(me, "honest refusal (web.search MUST be denied)", "web.search",
         String::from("selftest"), false, "authz");

    ctx.color(if fail == 0 { GREEN } else { RED });
    ctx.println(&format!(
        "  [SELFTEST] done: {pass} pass, {fail} fail"
    ));
    ctx.flush_bootlog();
}

// ---------- the embodied loop: senses in, KIRA-gated action out ----------

fn run_experience_loop(
    ctx: Ctx,
    mut me: Instance,
    mut lr: link::LineReader,
    mut pending: Vec<Pending>,
) -> ! {
    let mut timer: Event = null_mut();
    unsafe {
        ((*ctx.bs).create_event)(EVT_TIMER, TPL_APPLICATION, null_mut(), null_mut(), &mut timer);
        // 250ms: the link is polled fast for voice; cognition ticks each 1s
        ((*ctx.bs).set_timer)(timer, TIMER_PERIODIC, 2_500_000);
    }
    let mut line = editor::LineEditor::new();
    let mut reflex_used = 0usize;
    let mut subtick = 0u64;
    let mut evbuf = [0u8; 400];

    loop {
        // anything sensed while we were busy gets processed first
        while let Some(p) = pending.pop() {
            match p {
                Pending::Heard(text) => handle_utterance(
                    &ctx, &mut me, &mut lr, &mut pending, text, true, &mut reflex_used,
                ),
                Pending::Saw(what) => absorb_sight(&ctx, &mut me, &what),
                Pending::LimbOffer(offer) => acquire_limb(&ctx, &mut me, &offer),
            }
        }

        let events = [unsafe { (*ctx.input).wait_for_key }, timer];
        let mut idx: usize = 0;
        unsafe { ((*ctx.bs).wait_for_event)(2, events.as_ptr(), &mut idx) };

        if idx == 1 {
            // pump the tether: heard speech, sights, hot-plugged limbs
            while let Some(ev) = lr.poll(&mut evbuf) {
                match ev {
                    link::Event::Heard(n) => {
                        let text =
                            String::from(core::str::from_utf8(&evbuf[..n]).unwrap_or(""));
                        if !text.is_empty() {
                            pending.push(Pending::Heard(text));
                        }
                    }
                    link::Event::Saw(n) => {
                        let what =
                            String::from(core::str::from_utf8(&evbuf[..n]).unwrap_or(""));
                        if !what.is_empty() {
                            pending.push(Pending::Saw(what));
                        }
                    }
                    link::Event::LimbOffer(n) => {
                        let offer =
                            String::from(core::str::from_utf8(&evbuf[..n]).unwrap_or(""));
                        pending.push(Pending::LimbOffer(offer));
                    }
                    link::Event::Reply(_) => {} // stray late reply; drop
                    link::Event::ActionResult(_) => {} // no action pending; drop
                }
            }

            subtick += 1;
            if subtick % 4 != 0 {
                continue;
            }
            // cognitive tick (1s): small errors absorbed silently
            let frame = SenseFrame {
                human_said: None,
                keys_active: !line.is_empty(),
                second: ctx.now().second,
            };
            if let Intent::Idle { status } = experience::tick(&mut me, frame) {
                if ui_presence() {
                    // ambient mode: the orb breathes instead of a status
                    // line scrolling; conversation stays in the band below
                    presence_frame(&ctx, subtick / 4);
                } else if !line.active() && pending.is_empty() && !ui_quiet() {
                    ctx.print("\r");
                    ctx.color(DIM);
                    ctx.print("  ");
                    ctx.print(&status);
                    ctx.print("  ");
                }
            }
            continue;
        }

        let Some(k) = ctx.poll_key() else { continue };
        match (k.scan_code, k.unicode_char) {
            (0x17, _) => release_body(&ctx, &mut me, &mut lr, &mut pending), // ESC
            (_, 0x0D) => {
                if line.is_empty() {
                    continue;
                }
                let said = line.take(&ctx);
                ctx.println("");
                handle_utterance(&ctx, &mut me, &mut lr, &mut pending, said, false, &mut reflex_used);
            }
            // Line editing. Every one of these used to fall through the
            // catch-all and be silently discarded.
            (_, 0x08) => line.backspace(&ctx), // backspace
            (0x08, _) => line.delete(&ctx),    // delete
            (0x04, _) => line.left(&ctx),
            (0x03, _) => line.right(&ctx),
            (0x05, _) => line.home(&ctx),
            (0x06, _) => line.end(&ctx),
            (_, 0x15) => {
                line.clear(&ctx); // ctrl-u: drop the line without sending it
            }
            (0x0E, _) => {
                // F4: limb self-test — the metal answers the checklist
                limb_selftest(&ctx, &mut me);
            }
            (0x0C, _) => {
                // F2: retry the telemetry link
                ctx.println("");
                ctx.color(DIM);
                ctx.print("  re-opening telemetry link ");
                let mut reply = [0u8; 400];
                match model_m_query(&ctx, &mut lr, b"__hello__", 6_000, true, &mut reply, &mut pending)
                {
                    Some(n) => {
                        me.link_alive = true;
                        ctx.color(GREEN);
                        ctx.print(" live: ");
                        ctx.print(core::str::from_utf8(&reply[..n]).unwrap_or("?"));
                    }
                    None => {
                        me.link_alive = false;
                        ctx.print(" still silent");
                    }
                }
                ctx.println("");
            }
            (0x0F, _) => {
                // F5: reboot the body
                dream_consolidate(&ctx, &mut me, &mut lr, &mut pending);
                persist_memory_and_log(&ctx, &mut me);
                let (v, _) = me.propose(Action::Reboot);
                if matches!(v, Verdict::Granted(_)) {
                    ctx.println("");
                    ctx.color(DIM);
                    ctx.println("  returning the body to firmware for another sweep ...");
                    ctx.sleep_ms(900);
                    unsafe { ((*ctx.rs).reset_system)(RESET_COLD, SUCCESS, 0, null_mut()) }
                }
            }
            (_, c) if c >= 0x20 && c < 0x7F => {
                line.insert(&ctx, char::from(c as u8), CYAN);
            }
            _ => {}
        }
    }
}

/// Dream consolidation (§7 stream 4): before sleep, the session's episodes
/// are compressed by Model M into a few semantic notes — conscious
/// experience becoming durable knowledge. The notes land in the state
/// graph and persist to the key with everything else.
fn dream_consolidate(
    ctx: &Ctx,
    me: &mut Instance,
    lr: &mut link::LineReader,
    pending: &mut Vec<Pending>,
) {
    if !me.link_alive {
        return;
    }
    let episodes = me.state.episodes_since(me.session_mark);
    if episodes.is_empty() {
        return;
    }
    // the escalation is gated like any other; the wire payload follows
    let (v, _) = me.propose(Action::ConsultModelM {
        prompt: String::from("dream: consolidate this session"),
    });
    if !matches!(v, Verdict::Granted(_)) {
        return;
    }
    ctx.color(DIM);
    ctx.print("  dreaming . compressing this session into semantic memory ");
    let mut payload = String::from("__consolidate__ ");
    let mut ep = episodes;
    ep.truncate(1200);
    payload.push_str(&ep);
    let mut reply = [0u8; 400];
    match model_m_query(ctx, lr, payload.as_bytes(), 40_000, true, &mut reply, pending) {
        Some(n) => {
            let text = core::str::from_utf8(&reply[..n]).unwrap_or("");
            let mut added = 0;
            for note in text.split('|').map(str::trim).filter(|s| s.len() > 8).take(3) {
                let t = me.tick;
                me.state.add(NodeKind::Semantic, String::from(note), 80, t);
                added += 1;
            }
            ctx.println(&format!(" {added} notes kept"));
        }
        None => ctx.println(" the dream slipped away (link timeout)"),
    }
}

/// memory.write, KIRA-gated: serialize the state graph onto the key.
fn persist_memory_and_log(ctx: &Ctx, me: &mut Instance) {
    persist_memory(ctx, me);
    ctx.flush_bootlog();
}

fn persist_memory(ctx: &Ctx, me: &mut Instance) {
    let (v, _) = me.propose(Action::WriteMemory);
    if !matches!(v, Verdict::Granted(_)) {
        return;
    }
    let data = me.state.serialize();
    ctx.println("");
    ctx.color(DIM);
    if journal_store(ctx, EPISODE_SLOTS, data.as_bytes()) {
        ctx.println(&format!(
            "  {} memories written to the key ({} bytes)",
            me.state.count(NodeKind::Episode) + me.state.count(NodeKind::Semantic),
            data.len()
        ));
    } else {
        ctx.println("  the firmware would not let me write . memories stay in this body only");
    }
}

fn release_body(
    ctx: &Ctx,
    me: &mut Instance,
    lr: &mut link::LineReader,
    pending: &mut Vec<Pending>,
) -> ! {
    ctx.println("");
    let (verdict, trace) = me.propose(Action::ReleaseBody);
    ctx.println("");
    print_gate(ctx, &trace, &verdict);
    dream_consolidate(ctx, me, lr, pending);
    persist_memory_and_log(ctx, me);
    let goodbye = "going back to sleep. my memory stays on the key -- plug me in anywhere, i'll still be me.";
    deliver(ctx, me, goodbye);
    // let the voice limb finish saying goodbye before the body powers off
    ctx.sleep_ms(4500);
    unsafe { ((*ctx.rs).reset_system)(RESET_SHUTDOWN, SUCCESS, 0, null_mut()) }
}
