//! A real line editor for the console body region.
//!
//! Before this, typed input was append-only: every key that was not a
//! printable ASCII character fell through the match and was dropped, so
//! backspace did nothing and the arrow keys did nothing. A human could
//! not correct a typo — the only way out of a bad line was to send it.
//!
//! The editor keeps one line of input on one screen row and redraws that
//! row in place. Input longer than the row scrolls horizontally inside a
//! fixed window rather than wrapping, because a wrapped line cannot be
//! redrawn in place without disturbing the transcript scrolled above it.

use crate::console::Ctx;
use alloc::string::String;

/// Matches the previous input cap — a line the entity hears, not a document.
const MAX_LEN: usize = 200;

pub struct LineEditor {
    buf: String,
    /// Byte index of the caret. Input is ASCII-only (the console filters
    /// non-ASCII on the way in), so byte and character indices coincide.
    cur: usize,
    /// First visible byte — the horizontal scroll offset.
    off: usize,
    start_col: usize,
    row: usize,
    active: bool,
}

impl LineEditor {
    pub fn new() -> Self {
        LineEditor {
            buf: String::new(),
            cur: 0,
            off: 0,
            start_col: 0,
            row: 0,
            active: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// Open a fresh input line and anchor it where the cursor now sits.
    fn begin(&mut self, ctx: &Ctx, prompt_color: usize) {
        ctx.println("");
        ctx.color(prompt_color);
        ctx.print("  you: ");
        self.start_col = ctx.cursor_col();
        self.row = ctx.cursor_row();
        self.active = true;
        ctx.show_cursor(true);
    }

    /// Width available for text on the input row, less one column so the
    /// caret at end-of-line still has somewhere to sit.
    fn window(&self, ctx: &Ctx) -> usize {
        ctx.cols().saturating_sub(self.start_col + 1).max(8)
    }

    /// Keep the caret inside the visible window.
    fn rescroll(&mut self, ctx: &Ctx) {
        let w = self.window(ctx);
        if self.cur < self.off {
            self.off = self.cur;
        } else if self.cur >= self.off + w {
            self.off = self.cur + 1 - w;
        }
    }

    /// Repaint the input row and park the caret at the edit position.
    fn redraw(&self, ctx: &Ctx) {
        let w = self.window(ctx);
        let end = (self.off + w).min(self.buf.len());
        let visible = &self.buf[self.off..end];
        ctx.set_cursor(self.start_col, self.row);
        ctx.print(visible);
        // Erase whatever the previous, longer line left behind.
        let mut pad = w - visible.len();
        while pad > 0 {
            let chunk = pad.min(16);
            ctx.print(&"                "[..chunk]);
            pad -= chunk;
        }
        ctx.set_cursor(self.start_col + (self.cur - self.off), self.row);
    }

    pub fn insert(&mut self, ctx: &Ctx, c: char, prompt_color: usize) {
        if !self.active {
            self.begin(ctx, prompt_color);
        }
        if self.buf.len() >= MAX_LEN {
            return;
        }
        self.buf.insert(self.cur, c);
        self.cur += 1;
        self.rescroll(ctx);
        self.redraw(ctx);
    }

    pub fn backspace(&mut self, ctx: &Ctx) {
        if !self.active || self.cur == 0 {
            return;
        }
        self.cur -= 1;
        self.buf.remove(self.cur);
        self.rescroll(ctx);
        self.redraw(ctx);
    }

    pub fn delete(&mut self, ctx: &Ctx) {
        if !self.active || self.cur >= self.buf.len() {
            return;
        }
        self.buf.remove(self.cur);
        self.rescroll(ctx);
        self.redraw(ctx);
    }

    pub fn left(&mut self, ctx: &Ctx) {
        if !self.active || self.cur == 0 {
            return;
        }
        self.cur -= 1;
        self.rescroll(ctx);
        self.redraw(ctx);
    }

    pub fn right(&mut self, ctx: &Ctx) {
        if !self.active || self.cur >= self.buf.len() {
            return;
        }
        self.cur += 1;
        self.rescroll(ctx);
        self.redraw(ctx);
    }

    pub fn home(&mut self, ctx: &Ctx) {
        if !self.active {
            return;
        }
        self.cur = 0;
        self.rescroll(ctx);
        self.redraw(ctx);
    }

    pub fn end(&mut self, ctx: &Ctx) {
        if !self.active {
            return;
        }
        self.cur = self.buf.len();
        self.rescroll(ctx);
        self.redraw(ctx);
    }

    /// Wipe the line without sending it (the escape hatch a human expects
    /// from a terminal). Returns true if there was anything to clear.
    pub fn clear(&mut self, ctx: &Ctx) -> bool {
        if !self.active || self.buf.is_empty() {
            return false;
        }
        self.buf.clear();
        self.cur = 0;
        self.off = 0;
        self.redraw(ctx);
        ctx.set_cursor(self.start_col, self.row);
        true
    }

    /// Hand the finished line to cognition and close the input row.
    pub fn take(&mut self, ctx: &Ctx) -> String {
        self.active = false;
        self.cur = 0;
        self.off = 0;
        ctx.show_cursor(false);
        core::mem::take(&mut self.buf)
    }
}
