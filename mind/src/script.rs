//! A small deterministic interpreter — the entity's own compute (§9.2).
//!
//! `code.run` is a limb like any other: KIRA-gated, bounded, executed by
//! the core itself inside this body. Nothing here reaches a host, a
//! network, or even the firmware — a program is pure computation over an
//! environment, and its only observable effect is the output string the
//! entity then narrates. That is what makes the capability honest: the
//! entity really does run the code, on its own silicon, within a step
//! budget the gate can reason about.
//!
//! The language ("blur script") is small but genuinely programmable:
//!   let x = <expr>              bind or rebind a variable
//!   x = <expr>                  reassign; xs[i] = <expr> writes an element
//!   print <expr>, ...           append a line to the output
//!   if <expr> { } else { }      conditional (else-if chains allowed)
//!   while <expr> { }            loop while the condition holds
//!   repeat <expr> { }           run a block a fixed number of times
//!   fn name(a, b) { ... }       define a function; `return <expr>` exits it
//!   # ...                       comment to end of line
//! Values are 64-bit integers, strings, and lists. Operators: + - * / %,
//! == != < > <= >=, && || !, indexing `xs[i]`, and the builtins
//! len/push/str/upper/lower/contains. Statements separate on newlines
//! or `;`.
//!
//! Everything is bounded: steps, recursion depth, list length, string
//! length, and the output digest. A program that exceeds any bound stops
//! and says which one — never a hang, never a half-truth.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// Every statement executed and every expression node evaluated costs one
/// step. A program that exhausts this is stopped and says so — bounded
/// execution is part of the capability's contract, not a failure mode.
/// Sized so real algorithms (sorts, recursive series, string building)
/// finish comfortably while a runaway loop still dies in milliseconds.
const STEP_BUDGET: u32 = 200_000;
/// The digest crossing to the human is bounded (§ the AR contract).
const OUT_CAP: usize = 280;
/// Recursion is real but finite — the core runs on a firmware stack.
const MAX_DEPTH: usize = 64;
/// Memory ceilings, so a program cannot eat the entity's pool.
const MAX_LIST: usize = 4096;
const MAX_STR: usize = 4096;

// ---------- values ----------

#[derive(Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Str(String),
    List(Vec<Value>),
}

impl Value {
    fn show(&self) -> String {
        match self {
            Value::Int(v) => v.to_string(),
            Value::Str(s) => s.clone(),
            Value::List(items) => {
                let mut s = String::from("[");
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&it.show());
                    if s.len() > OUT_CAP {
                        s.push_str("...");
                        break;
                    }
                }
                s.push(']');
                s
            }
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Value::Int(v) => *v != 0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Value::Int(_) => "a number",
            Value::Str(_) => "a string",
            Value::List(_) => "a list",
        }
    }
}

// ---------- tokens ----------

#[derive(Clone, PartialEq)]
enum Tok {
    Num(i64),
    Ident(String),
    Str(String),
    Op(String),
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' | '\r' => i += 1,
            '\n' => {
                out.push(Tok::Op(String::from(";")));
                i += 1;
            }
            '#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            '0'..='9' => {
                let mut v: i64 = 0;
                while i < b.len() && b[i].is_ascii_digit() {
                    v = v
                        .checked_mul(10)
                        .and_then(|v| v.checked_add((b[i] - b'0') as i64))
                        .ok_or_else(|| String::from("number too large"))?;
                    i += 1;
                }
                out.push(Tok::Num(v));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let s = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(Tok::Ident(src[s..i].to_lowercase()));
            }
            '"' | '\'' => {
                let q = b[i];
                i += 1;
                let s = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                if i >= b.len() {
                    return Err(String::from("unterminated string"));
                }
                out.push(Tok::Str(src[s..i].to_string()));
                i += 1;
            }
            _ => {
                if !b[i].is_ascii() {
                    return Err(format!("unexpected character '{c}'"));
                }
                // longest match first, so '<=' never reads as '<' then '='
                if i + 1 < b.len() && b[i + 1].is_ascii() {
                    let mut two = String::new();
                    two.push(c);
                    two.push(b[i + 1] as char);
                    if matches!(two.as_str(), "==" | "!=" | "<=" | ">=" | "&&" | "||") {
                        out.push(Tok::Op(two));
                        i += 2;
                        continue;
                    }
                }
                if "+-*/%()[]{}=,;<>!".contains(c) {
                    out.push(Tok::Op(String::from(c)));
                    i += 1;
                } else {
                    return Err(format!("unexpected character '{c}'"));
                }
            }
        }
    }
    Ok(out)
}

// ---------- ast ----------

enum Expr {
    Lit(Value),
    Var(String),
    Unary(char, Box<Expr>),
    Bin(String, Box<Expr>, Box<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    ListLit(Vec<Expr>),
}

enum Stmt {
    Let(String, Expr),
    Assign(String, Expr),
    SetIndex(String, Expr, Expr),
    Print(Vec<Expr>),
    Repeat(Expr, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    Fn(String, Vec<String>, Vec<Stmt>),
    Return(Option<Expr>),
    Discard(Expr),
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn peek_op(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Op(o)) if o == s)
    }

    fn eat_op(&mut self, s: &str) -> bool {
        if self.peek_op(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    // precedence climb, loosest binding first
    fn expr(&mut self) -> Result<Expr, String> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.and_expr()?;
        while self.peek_op("||") {
            self.i += 1;
            l = Expr::Bin(String::from("||"), Box::new(l), Box::new(self.and_expr()?));
        }
        Ok(l)
    }

    fn and_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.eq_expr()?;
        while self.peek_op("&&") {
            self.i += 1;
            l = Expr::Bin(String::from("&&"), Box::new(l), Box::new(self.eq_expr()?));
        }
        Ok(l)
    }

    fn eq_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.cmp_expr()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "==" || o == "!=" => o.clone(),
                _ => break,
            };
            self.i += 1;
            l = Expr::Bin(op, Box::new(l), Box::new(self.cmp_expr()?));
        }
        Ok(l)
    }

    fn cmp_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.add_expr()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "<" || o == ">" || o == "<=" || o == ">=" => o.clone(),
                _ => break,
            };
            self.i += 1;
            l = Expr::Bin(op, Box::new(l), Box::new(self.add_expr()?));
        }
        Ok(l)
    }

    fn add_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "+" || o == "-" => o.clone(),
                _ => break,
            };
            self.i += 1;
            l = Expr::Bin(op, Box::new(l), Box::new(self.mul_expr()?));
        }
        Ok(l)
    }

    fn mul_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if o == "*" || o == "/" || o == "%" => o.clone(),
                _ => break,
            };
            self.i += 1;
            l = Expr::Bin(op, Box::new(l), Box::new(self.unary()?));
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat_op("-") {
            return Ok(Expr::Unary('-', Box::new(self.unary()?)));
        }
        if self.eat_op("!") {
            return Ok(Expr::Unary('!', Box::new(self.unary()?)));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        while self.peek_op("[") {
            self.i += 1;
            let idx = self.expr()?;
            if !self.eat_op("]") {
                return Err(String::from("missing ']'"));
            }
            e = Expr::Index(Box::new(e), Box::new(idx));
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(Tok::Num(v)) => {
                let v = *v;
                self.i += 1;
                Ok(Expr::Lit(Value::Int(v)))
            }
            Some(Tok::Str(s)) => {
                let s = s.clone();
                self.i += 1;
                Ok(Expr::Lit(Value::Str(s)))
            }
            Some(Tok::Ident(name)) => {
                let name = name.clone();
                self.i += 1;
                if self.eat_op("(") {
                    let mut args = Vec::new();
                    if !self.peek_op(")") {
                        loop {
                            args.push(self.expr()?);
                            if !self.eat_op(",") {
                                break;
                            }
                        }
                    }
                    if !self.eat_op(")") {
                        return Err(format!("missing ')' after arguments to {name}"));
                    }
                    return Ok(Expr::Call(name, args));
                }
                Ok(Expr::Var(name))
            }
            Some(Tok::Op(o)) if o == "(" => {
                self.i += 1;
                let e = self.expr()?;
                if !self.eat_op(")") {
                    return Err(String::from("missing ')'"));
                }
                Ok(e)
            }
            Some(Tok::Op(o)) if o == "[" => {
                self.i += 1;
                let mut items = Vec::new();
                if !self.peek_op("]") {
                    loop {
                        items.push(self.expr()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
                if !self.eat_op("]") {
                    return Err(String::from("missing ']' in list"));
                }
                Ok(Expr::ListLit(items))
            }
            _ => Err(String::from("expected a number, string, variable or '('")),
        }
    }

    fn braced_block(&mut self, what: &str) -> Result<Vec<Stmt>, String> {
        if !self.eat_op("{") {
            return Err(format!("{what} needs '{{'"));
        }
        let body = self.block()?;
        if !self.eat_op("}") {
            return Err(format!("{what} block missing '}}'"));
        }
        Ok(body)
    }

    fn block(&mut self) -> Result<Vec<Stmt>, String> {
        let mut out = Vec::new();
        loop {
            while self.eat_op(";") {}
            match self.peek() {
                None => return Ok(out),
                Some(Tok::Op(o)) if o == "}" => return Ok(out),
                _ => out.push(self.stmt()?),
            }
        }
    }

    fn stmt(&mut self) -> Result<Stmt, String> {
        let Some(Tok::Ident(kw)) = self.peek() else {
            return Err(String::from(
                "expected a statement (let / print / if / while / repeat / fn / return)",
            ));
        };
        let kw = kw.clone();
        match kw.as_str() {
            "let" => {
                self.i += 1;
                let Some(Tok::Ident(name)) = self.peek() else {
                    return Err(String::from("let needs a variable name"));
                };
                let name = name.clone();
                self.i += 1;
                if !self.eat_op("=") {
                    return Err(String::from("let needs '='"));
                }
                Ok(Stmt::Let(name, self.expr()?))
            }
            "print" => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    items.push(self.expr()?);
                    if !self.eat_op(",") {
                        break;
                    }
                }
                Ok(Stmt::Print(items))
            }
            "repeat" => {
                self.i += 1;
                let n = self.expr()?;
                Ok(Stmt::Repeat(n, self.braced_block("repeat")?))
            }
            "while" => {
                self.i += 1;
                let c = self.expr()?;
                Ok(Stmt::While(c, self.braced_block("while")?))
            }
            "if" => {
                self.i += 1;
                let c = self.expr()?;
                let then = self.braced_block("if")?;
                let mut otherwise = Vec::new();
                // `else` may start a block or chain into another `if`
                if matches!(self.peek(), Some(Tok::Ident(k)) if k == "else") {
                    self.i += 1;
                    if matches!(self.peek(), Some(Tok::Ident(k)) if k == "if") {
                        otherwise = vec![self.stmt()?];
                    } else {
                        otherwise = self.braced_block("else")?;
                    }
                }
                Ok(Stmt::If(c, then, otherwise))
            }
            "fn" => {
                self.i += 1;
                let Some(Tok::Ident(name)) = self.peek() else {
                    return Err(String::from("fn needs a name"));
                };
                let name = name.clone();
                self.i += 1;
                if !self.eat_op("(") {
                    return Err(String::from("fn needs '(' after its name"));
                }
                let mut params = Vec::new();
                if !self.peek_op(")") {
                    loop {
                        let Some(Tok::Ident(p)) = self.peek() else {
                            return Err(String::from("fn parameters must be names"));
                        };
                        params.push(p.clone());
                        self.i += 1;
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
                if !self.eat_op(")") {
                    return Err(String::from("fn parameter list missing ')'"));
                }
                Ok(Stmt::Fn(name, params, self.braced_block("fn")?))
            }
            "return" => {
                self.i += 1;
                // `return` alone is valid; so is `return <expr>`
                let done = match self.peek() {
                    None => true,
                    Some(Tok::Op(o)) => o == ";" || o == "}",
                    _ => false,
                };
                Ok(Stmt::Return(if done { None } else { Some(self.expr()?) }))
            }
            _ => {
                // an assignment, an indexed write, or a bare call
                let name = kw.clone();
                let save = self.i;
                self.i += 1;
                if self.eat_op("=") {
                    return Ok(Stmt::Assign(name, self.expr()?));
                }
                if self.peek_op("[") {
                    let probe = self.i;
                    self.i += 1;
                    if let Ok(idx) = self.expr() {
                        if self.eat_op("]") && self.eat_op("=") {
                            return Ok(Stmt::SetIndex(name, idx, self.expr()?));
                        }
                    }
                    self.i = probe;
                }
                self.i = save;
                Ok(Stmt::Discard(self.expr()?))
            }
        }
    }
}

// ---------- execution ----------

enum Flow {
    Normal,
    Return(Value),
}

struct Func {
    params: Vec<String>,
    body: Vec<Stmt>,
}

struct Env {
    frames: Vec<Vec<(String, Value)>>,
    fns: Vec<(String, Rc<Func>)>,
    steps: u32,
    out: String,
    depth: usize,
}

impl Env {
    fn spend(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            Err(String::from("step budget exhausted"))
        } else {
            Ok(())
        }
    }

    fn frame(&mut self) -> &mut Vec<(String, Value)> {
        self.frames.last_mut().expect("a frame is always open")
    }

    fn get(&self, name: &str) -> Option<&Value> {
        self.frames
            .last()
            .and_then(|f| f.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v))
    }

    fn set(&mut self, name: &str, v: Value) {
        let f = self.frame();
        if let Some(slot) = f.iter_mut().find(|(k, _)| k == name) {
            slot.1 = v;
        } else {
            f.push((String::from(name), v));
        }
    }

    fn eval(&mut self, e: &Expr) -> Result<Value, String> {
        self.spend()?;
        match e {
            Expr::Lit(v) => Ok(v.clone()),
            Expr::Var(n) => self
                .get(n)
                .cloned()
                .ok_or_else(|| format!("unknown variable '{n}'")),
            Expr::Unary('-', x) => match self.eval(x)? {
                Value::Int(v) => v.checked_neg().map(Value::Int).ok_or_else(overflow),
                other => Err(format!("cannot negate {}", other.kind())),
            },
            Expr::Unary(_, x) => {
                let v = self.eval(x)?;
                Ok(Value::Int(if v.truthy() { 0 } else { 1 }))
            }
            Expr::Bin(op, l, r) if op == "&&" || op == "||" => {
                // short-circuit: the right side is not evaluated when the
                // left already decides the answer
                let lv = self.eval(l)?.truthy();
                let decided = if op == "&&" { !lv } else { lv };
                if decided {
                    return Ok(Value::Int(lv as i64));
                }
                Ok(Value::Int(self.eval(r)?.truthy() as i64))
            }
            Expr::Bin(op, l, r) => {
                let (lv, rv) = (self.eval(l)?, self.eval(r)?);
                binop(op, lv, rv)
            }
            Expr::Index(target, idx) => {
                let t = self.eval(target)?;
                let i = match self.eval(idx)? {
                    Value::Int(i) => i,
                    other => return Err(format!("an index must be a number, not {}", other.kind())),
                };
                match t {
                    Value::List(items) => index_of(i, items.len())
                        .map(|u| items[u].clone())
                        .ok_or_else(|| format!("index {i} is outside a list of {}", items.len())),
                    Value::Str(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        index_of(i, chars.len())
                            .map(|u| Value::Str(chars[u].to_string()))
                            .ok_or_else(|| {
                                format!("index {i} is outside a string of {}", chars.len())
                            })
                    }
                    other => Err(format!("cannot index {}", other.kind())),
                }
            }
            Expr::ListLit(items) => {
                if items.len() > MAX_LIST {
                    return Err(String::from("that list is too long to hold"));
                }
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval(it)?);
                }
                Ok(Value::List(out))
            }
            Expr::Call(name, args) => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a)?);
                }
                if let Some(v) = builtin(name, &vals)? {
                    return Ok(v);
                }
                self.call(name, vals)
            }
        }
    }

    fn call(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        // the definition is shared, not copied: recursion would otherwise
        // clone the whole body once per call
        let f = self
            .fns
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, f)| Rc::clone(f))
            .ok_or_else(|| format!("there's no function called '{name}'"))?;
        if f.params.len() != args.len() {
            return Err(format!(
                "'{name}' wants {} argument(s), got {}",
                f.params.len(),
                args.len()
            ));
        }
        if self.depth >= MAX_DEPTH {
            return Err(String::from("recursion went too deep"));
        }
        let mut frame = Vec::with_capacity(f.params.len());
        for (p, v) in f.params.iter().zip(args) {
            frame.push((p.clone(), v));
        }
        self.frames.push(frame);
        self.depth += 1;
        let flow = self.exec(&f.body);
        self.frames.pop();
        self.depth -= 1;
        match flow? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Ok(Value::Int(0)),
        }
    }

    fn emit(&mut self, s: &str) {
        if self.out.len() < OUT_CAP {
            if !self.out.is_empty() {
                self.out.push_str(" | ");
            }
            let room = OUT_CAP.saturating_sub(self.out.len());
            let mut end = s.len().min(room);
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            self.out.push_str(&s[..end]);
        }
    }

    fn exec(&mut self, stmts: &[Stmt]) -> Result<Flow, String> {
        for s in stmts {
            self.spend()?;
            match s {
                Stmt::Fn(..) => {} // hoisted before execution
                Stmt::Let(n, e) | Stmt::Assign(n, e) => {
                    let v = self.eval(e)?;
                    if matches!(s, Stmt::Assign(..)) && self.get(n).is_none() {
                        return Err(format!("unknown variable '{n}'"));
                    }
                    self.set(n, v);
                }
                Stmt::SetIndex(name, idx, val) => {
                    let i = match self.eval(idx)? {
                        Value::Int(i) => i,
                        other => {
                            return Err(format!(
                                "an index must be a number, not {}",
                                other.kind()
                            ))
                        }
                    };
                    let v = self.eval(val)?;
                    let cur = self
                        .get(name)
                        .cloned()
                        .ok_or_else(|| format!("unknown variable '{name}'"))?;
                    let Value::List(mut items) = cur else {
                        return Err(format!("'{name}' is not a list"));
                    };
                    let u = index_of(i, items.len())
                        .ok_or_else(|| format!("index {i} is outside a list of {}", items.len()))?;
                    items[u] = v;
                    self.set(name, Value::List(items));
                }
                Stmt::Print(items) => {
                    let mut line = String::new();
                    for it in items {
                        if !line.is_empty() {
                            line.push(' ');
                        }
                        line.push_str(&self.eval(it)?.show());
                        if line.len() > OUT_CAP {
                            break;
                        }
                    }
                    self.emit(&line);
                }
                Stmt::Discard(e) => {
                    self.eval(e)?;
                }
                Stmt::Repeat(n, body) => {
                    let n = match self.eval(n)? {
                        Value::Int(v) => v.clamp(0, 1_000_000),
                        other => {
                            return Err(format!("repeat needs a number, not {}", other.kind()))
                        }
                    };
                    for _ in 0..n {
                        if let Flow::Return(v) = self.exec(body)? {
                            return Ok(Flow::Return(v));
                        }
                    }
                }
                Stmt::While(cond, body) => {
                    while self.eval(cond)?.truthy() {
                        self.spend()?;
                        if let Flow::Return(v) = self.exec(body)? {
                            return Ok(Flow::Return(v));
                        }
                    }
                }
                Stmt::If(cond, then, otherwise) => {
                    let branch = if self.eval(cond)?.truthy() { then } else { otherwise };
                    if let Flow::Return(v) = self.exec(branch)? {
                        return Ok(Flow::Return(v));
                    }
                }
                Stmt::Return(e) => {
                    let v = match e {
                        Some(e) => self.eval(e)?,
                        None => Value::Int(0),
                    };
                    return Ok(Flow::Return(v));
                }
            }
        }
        Ok(Flow::Normal)
    }
}

/// Negative indices count from the end, the way a person counts backwards.
fn index_of(i: i64, len: usize) -> Option<usize> {
    let idx = if i < 0 { i + len as i64 } else { i };
    if idx < 0 || idx as usize >= len {
        None
    } else {
        Some(idx as usize)
    }
}

fn binop(op: &str, l: Value, r: Value) -> Result<Value, String> {
    use Value::*;
    match op {
        "==" => return Ok(Int((l == r) as i64)),
        "!=" => return Ok(Int((l != r) as i64)),
        _ => {}
    }
    match (op, l, r) {
        ("+", Int(a), Int(b)) => a.checked_add(b).map(Int).ok_or_else(overflow),
        ("-", Int(a), Int(b)) => a.checked_sub(b).map(Int).ok_or_else(overflow),
        ("*", Int(a), Int(b)) => a.checked_mul(b).map(Int).ok_or_else(overflow),
        ("/", _, Int(0)) => Err(String::from("divide by zero")),
        ("/", Int(a), Int(b)) => a.checked_div(b).map(Int).ok_or_else(overflow),
        ("%", _, Int(0)) => Err(String::from("modulo by zero")),
        ("%", Int(a), Int(b)) => a.checked_rem(b).map(Int).ok_or_else(overflow),
        // a string on either side of '+' joins; a list joins with a list
        ("+", Str(a), b) => join_str(a, &b.show()),
        ("+", a, Str(b)) => join_str(a.show(), &b),
        ("+", List(a), List(b)) => {
            if a.len() + b.len() > MAX_LIST {
                return Err(String::from("that list is too long to hold"));
            }
            let mut out = a;
            out.extend(b);
            Ok(List(out))
        }
        ("<", a, b) => cmp(a, b, |o| o < 0),
        (">", a, b) => cmp(a, b, |o| o > 0),
        ("<=", a, b) => cmp(a, b, |o| o <= 0),
        (">=", a, b) => cmp(a, b, |o| o >= 0),
        (op, a, b) => Err(format!("cannot use '{op}' on {} and {}", a.kind(), b.kind())),
    }
}

fn join_str(mut a: String, b: &str) -> Result<Value, String> {
    if a.len() + b.len() > MAX_STR {
        return Err(String::from("that string is too long to hold"));
    }
    a.push_str(b);
    Ok(Value::Str(a))
}

fn cmp(a: Value, b: Value, keep: fn(i32) -> bool) -> Result<Value, String> {
    let ord = match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => (*x).cmp(y),
        (Value::Str(x), Value::Str(y)) => x.as_str().cmp(y.as_str()),
        _ => {
            return Err(format!(
                "cannot compare {} with {}",
                a.kind(),
                b.kind()
            ))
        }
    };
    let n = match ord {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    };
    Ok(Value::Int(keep(n) as i64))
}

/// The builtin library. Returns `None` when the name is not a builtin, so
/// a user function of the same shape is tried next.
fn builtin(name: &str, args: &[Value]) -> Result<Option<Value>, String> {
    let need = |n: usize| -> Result<(), String> {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!("'{name}' wants {n} argument(s), got {}", args.len()))
        }
    };
    let v = match name {
        "len" => {
            need(1)?;
            match &args[0] {
                Value::List(l) => Value::Int(l.len() as i64),
                Value::Str(s) => Value::Int(s.chars().count() as i64),
                other => return Err(format!("len needs a list or string, not {}", other.kind())),
            }
        }
        "push" => {
            need(2)?;
            let Value::List(l) = &args[0] else {
                return Err(format!("push needs a list, not {}", args[0].kind()));
            };
            if l.len() + 1 > MAX_LIST {
                return Err(String::from("that list is too long to hold"));
            }
            let mut out = l.clone();
            out.push(args[1].clone());
            Value::List(out)
        }
        "str" => {
            need(1)?;
            Value::Str(args[0].show())
        }
        "upper" | "lower" => {
            need(1)?;
            let s = args[0].show();
            Value::Str(if name == "upper" {
                s.to_uppercase()
            } else {
                s.to_lowercase()
            })
        }
        "contains" => {
            need(2)?;
            match &args[0] {
                Value::Str(s) => Value::Int(s.contains(&args[1].show()) as i64),
                Value::List(l) => Value::Int(l.contains(&args[1]) as i64),
                other => {
                    return Err(format!(
                        "contains needs a string or list, not {}",
                        other.kind()
                    ))
                }
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(v))
}

fn overflow() -> String {
    String::from("integer overflow")
}

/// Collect function definitions before execution, so a program may call a
/// function defined further down — the way a person writes helpers last.
fn hoist(stmts: &[Stmt], out: &mut Vec<(String, Rc<Func>)>) {
    for s in stmts {
        if let Stmt::Fn(name, params, body) = s {
            hoist(body, out);
            let f = Rc::new(Func { params: params.clone(), body: clone_body(body) });
            if let Some(slot) = out.iter_mut().find(|(n, _)| n == name) {
                slot.1 = f;
            } else {
                out.push((name.clone(), f));
            }
        }
    }
}

// `Stmt` holds boxed expressions, so a manual deep clone keeps the AST
// free of a blanket Clone derive that would invite accidental copies.
fn clone_body(body: &[Stmt]) -> Vec<Stmt> {
    body.iter().map(clone_stmt).collect()
}

fn clone_stmt(s: &Stmt) -> Stmt {
    match s {
        Stmt::Let(n, e) => Stmt::Let(n.clone(), clone_expr(e)),
        Stmt::Assign(n, e) => Stmt::Assign(n.clone(), clone_expr(e)),
        Stmt::SetIndex(n, i, v) => Stmt::SetIndex(n.clone(), clone_expr(i), clone_expr(v)),
        Stmt::Print(items) => Stmt::Print(items.iter().map(clone_expr).collect()),
        Stmt::Repeat(n, b) => Stmt::Repeat(clone_expr(n), clone_body(b)),
        Stmt::While(c, b) => Stmt::While(clone_expr(c), clone_body(b)),
        Stmt::If(c, a, b) => Stmt::If(clone_expr(c), clone_body(a), clone_body(b)),
        Stmt::Fn(n, p, b) => Stmt::Fn(n.clone(), p.clone(), clone_body(b)),
        Stmt::Return(e) => Stmt::Return(e.as_ref().map(clone_expr)),
        Stmt::Discard(e) => Stmt::Discard(clone_expr(e)),
    }
}

fn clone_expr(e: &Expr) -> Expr {
    match e {
        Expr::Lit(v) => Expr::Lit(v.clone()),
        Expr::Var(n) => Expr::Var(n.clone()),
        Expr::Unary(o, x) => Expr::Unary(*o, Box::new(clone_expr(x))),
        Expr::Bin(o, l, r) => {
            Expr::Bin(o.clone(), Box::new(clone_expr(l)), Box::new(clone_expr(r)))
        }
        Expr::Index(t, i) => Expr::Index(Box::new(clone_expr(t)), Box::new(clone_expr(i))),
        Expr::Call(n, a) => Expr::Call(n.clone(), a.iter().map(clone_expr).collect()),
        Expr::ListLit(items) => Expr::ListLit(items.iter().map(clone_expr).collect()),
    }
}

/// Run a program. Returns (ok, digest): the digest is either the bounded
/// output plus a step count, or a plain-language error — the entity
/// narrates whichever really happened.
pub fn run(src: &str) -> (bool, String) {
    let stmts = match tokenize(src).and_then(|t| {
        let mut p = Parser { t, i: 0 };
        let b = p.block()?;
        if p.peek().is_some() {
            return Err(String::from("unexpected '}'"));
        }
        Ok(b)
    }) {
        Ok(s) => s,
        Err(e) => return (false, format!("the program doesn't parse: {e}")),
    };
    if stmts.is_empty() {
        return (false, String::from("the program is empty"));
    }
    let mut fns = Vec::new();
    hoist(&stmts, &mut fns);
    let mut env = Env {
        frames: vec![Vec::new()],
        fns,
        steps: 0,
        out: String::new(),
        depth: 0,
    };
    match env.exec(&stmts) {
        Ok(_) => {
            let out = if env.out.is_empty() {
                String::from("(no output)")
            } else {
                env.out
            };
            (true, format!("output: {out} ({} steps)", env.steps))
        }
        Err(e) => (false, format!("the program failed while running: {e}")),
    }
}
