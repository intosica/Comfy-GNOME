// SPDX-License-Identifier: GPL-3.0-only
//! Setting *definitions*, pulled out of the JavaScript the backend serves.
//!
//! `GET /api/settings` only stores values. The web UI learns names, categories, types and
//! options from JS: the core list lives in a frontend chunk (`assets/GraphView-*.js` in
//! 1.45), the rest in every extension's `settings: [...]` / `addSetting({...})`. So we fetch
//! that JS and run a small tolerant tokenizer over it, keeping object literals that look like
//! a setting (`id` string + `type` key). Category path = `category || id.split('.')`, exactly
//! like `useSettingUI.ts`. Blocking network — call from `gio::spawn_blocking`.

use crate::api::Client;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct SettingDef {
    pub id: String,
    pub name: String,
    /// `[category, section, …, leaf]`; one element means "Other".
    pub path: Vec<String>,
    pub tooltip: String,
    /// `boolean`, `number`, `slider`, `combo`, `text`, `color`, `url`, … or `custom` (a JS function).
    pub kind: String,
    /// `(label, value)`; plain string options have both equal.
    pub options: Vec<(String, Value)>,
    pub default: Value,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

impl SettingDef {
    pub fn category(&self) -> &str {
        if self.path.len() >= 2 { &self.path[0] } else { "Other" }
    }
    pub fn section(&self) -> &str {
        if self.path.len() >= 3 { &self.path[1] } else { "" }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub defs: Vec<SettingDef>,
    /// `type: 'hidden'` ids: never shown, and not re-added from `/api/settings`.
    pub hidden: HashSet<String>,
    /// Sidebar order.
    pub categories: Vec<String>,
    /// `(pack, extension name)` from `registerExtension({name})`, load order; pack "Core" = frontend.
    pub extensions: Vec<(String, String)>,
}

/// Web UI labels that differ from the category key (`settingsCategories` in the i18n file).
pub fn category_label(c: &str) -> &str {
    match c {
        "LiteGraph" => "Lite Graph",
        c => c,
    }
}

const CORE_ORDER: [&str; 4] = ["Comfy", "LiteGraph", "Appearance", "Mask Editor"];

static CACHE: Mutex<Option<Arc<Catalog>>> = Mutex::new(None);

/// Definitions for this backend, built once per process (an empty result isn't cached).
pub fn catalog(c: &Client) -> Arc<Catalog> {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cat) = g.as_ref() {
        return cat.clone();
    }
    let cat = Arc::new(build(c));
    if !cat.defs.is_empty() {
        *g = Some(cat.clone());
    }
    cat
}

fn build(c: &Client) -> Catalog {
    let mut b = Builder::default();
    for (_, text) in frontend_chunks(c) {
        scan_safely(&mut b, &text, "Core", true);
    }
    let paths: Vec<String> = c.get("/extensions").ok().and_then(|v| v.as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_owned)).collect())).unwrap_or_default();
    for (p, text) in paths.iter().zip(fetch_all(c, &paths)) {
        let pack = p.trim_start_matches("/extensions/").split('/').next().unwrap_or("").to_owned();
        if let Some(text) = text {
            scan_safely(&mut b, &text, &pack, false);
        }
    }
    b.finish()
}

/// The scanner is a hand-written tokenizer over arbitrary third-party JS: one bad file is skipped, not fatal.
fn scan_safely(b: &mut Builder, text: &str, pack: &str, frontend: bool) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.scan(text, pack, frontend)));
}

/// Frontend chunks reachable from `index.html`, breadth first (bounded). Vendor and i18n
/// bundles never hold definitions and are most of the bytes, so they're skipped.
fn frontend_chunks(c: &Client) -> Vec<(String, String)> {
    const SKIP: [&str; 4] = ["vendor-", "nodeDefs-", "settings-", "commands-"];
    const MAX_FILES: usize = 400;
    const MAX_BYTES: usize = 24 << 20;
    let Some(index) = c.view_query("/").ok().map(|b| String::from_utf8_lossy(&b).into_owned()) else { return Vec::new() };
    let re = regex::Regex::new(r"(?:\./|assets/)([A-Za-z0-9_.\-]+\.js)\b").unwrap();
    let refs = |s: &str| -> Vec<String> { re.captures_iter(s).map(|m| m[1].to_owned()).filter(|n| !SKIP.iter().any(|p| n.starts_with(p))).collect() };
    let mut seen: HashSet<String> = HashSet::new();
    let mut frontier: Vec<String> = refs(&index).into_iter().filter(|n| seen.insert(n.clone())).collect();
    let (mut out, mut bytes) = (Vec::new(), 0);
    while !frontier.is_empty() && out.len() < MAX_FILES && bytes < MAX_BYTES {
        frontier.truncate(MAX_FILES - out.len());
        let paths: Vec<String> = frontier.iter().map(|n| format!("/assets/{n}")).collect();
        let mut next = Vec::new();
        for (name, text) in frontier.drain(..).zip(fetch_all(c, &paths)) {
            let Some(text) = text else { continue };
            next.extend(refs(&text).into_iter().filter(|n| seen.insert(n.clone())));
            bytes += text.len();
            out.push((name, text));
        }
        frontier = next;
    }
    out
}

/// GET many paths on 8 threads; results keep the input order.
fn fetch_all(c: &Client, paths: &[String]) -> Vec<Option<String>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    // ponytail: 30 s budget per batch; slower backends get a partial catalog (unfetched files = no defs).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let out: Mutex<Vec<Option<String>>> = Mutex::new(vec![None; paths.len()]);
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(p) = paths.get(i) else { break };
                if std::time::Instant::now() > deadline {
                    break;
                }
                let text = c.view_query(&encode_path(p)).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
                out.lock().unwrap()[i] = text;
            });
        }
    });
    out.into_inner().unwrap()
}

fn encode_path(p: &str) -> String {
    let mut s = String::new();
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Keys from `/api/settings` without a definition get a generic one (value type → editor),
/// grouped by id prefix. Returns the catalog to render.
pub fn merge(cat: &Catalog, values: &BTreeMap<String, Value>) -> Catalog {
    let mut out = cat.clone();
    let known: HashSet<&str> = cat.defs.iter().map(|d| d.id.as_str()).collect();
    for (k, v) in values {
        // ComfyGNOME.* are this app's own keys, drawn by workspace/settings.rs.
        if known.contains(k.as_str()) || cat.hidden.contains(k) || k.starts_with("ComfyGNOME.") {
            continue;
        }
        let kind = match v {
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "text",
            _ => "custom",
        };
        let path: Vec<String> = k.split('.').map(str::to_owned).collect();
        out.defs.push(SettingDef { id: k.clone(), name: k.clone(), path, tooltip: String::new(), kind: kind.into(), options: Vec::new(), default: Value::Null, min: None, max: None, step: None });
    }
    let mut cats = out.categories.clone();
    let other = cats.last().is_some_and(|c| c == "Other");
    if other {
        cats.pop();
    }
    for d in &out.defs[cat.defs.len()..] {
        if !cats.iter().any(|c| c == d.category()) && (d.category() != "Other" || !other) {
            cats.push(d.category().to_owned());
        }
    }
    if other {
        cats.push("Other".into());
    }
    out.categories = cats;
    out
}

#[derive(Default)]
struct Builder {
    defs: Vec<SettingDef>,
    ids: HashSet<String>,
    hidden: HashSet<String>,
    /// First-seen order of categories, with "came from the frontend".
    cats: Vec<(String, bool)>,
    extensions: Vec<(String, String)>,
}

impl Builder {
    fn scan(&mut self, src: &str, pack: &str, frontend: bool) {
        if !src.contains("id") {
            return;
        }
        let js = Js::new(src);
        let (defs, names) = js.extract();
        for n in names {
            if !self.extensions.iter().any(|(_, e)| *e == n) {
                self.extensions.push((pack.to_owned(), n));
            }
        }
        for d in defs {
            // The frontend also ships server-config fields (`cuda-malloc`, …) in the same
            // shape, and desktop-only settings the browser never registers.
            if frontend && (!d.id.contains('.') || d.category() == "Comfy-Desktop") {
                continue;
            }
            if !self.ids.insert(d.id.clone()) {
                continue;
            }
            if d.kind == "hidden" {
                self.hidden.insert(d.id);
                continue;
            }
            let c = d.category().to_owned();
            if !self.cats.iter().any(|(x, _)| *x == c) {
                self.cats.push((c, frontend));
            }
            self.defs.push(d);
        }
    }

    fn finish(self) -> Catalog {
        let has = |c: &str| self.cats.iter().any(|(x, _)| x == c);
        let mut categories: Vec<String> = CORE_ORDER.iter().filter(|c| has(c)).map(|c| c.to_string()).collect();
        let rest = |fe: bool| self.cats.iter().filter(move |(c, f)| *f == fe && !CORE_ORDER.contains(&c.as_str()) && c != "Other").map(|(c, _)| c.clone());
        // ponytail: extension load order is our /extensions order; the web registers them
        // asynchronously, and lazily loaded frontend extras (3D) land wherever they finish.
        categories.extend(rest(false));
        categories.extend(rest(true));
        if has("Other") {
            categories.push("Other".into());
        }
        Catalog { defs: self.defs, hidden: self.hidden, categories, extensions: self.extensions }
    }
}

// ---------------------------------------------------------------------------------------
// Tolerant JS tokenizer + literal reader.

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Str(String),
    Num(f64),
    Ident(String),
    P(u8),
    /// A regex or a template with `${}`: a value, but not a literal.
    Opaque,
}

fn lex(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut t: Vec<Tok> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = src[i + 2..].find("*/").map_or(b.len(), |p| i + 2 + p + 2);
        } else if c == b'/' && regex_allowed(t.last()) {
            match skip_regex(b, i) {
                Some(e) => {
                    t.push(Tok::Opaque);
                    i = e;
                }
                None => {
                    t.push(Tok::P(c));
                    i += 1;
                }
            }
        } else if c == b'"' || c == b'\'' {
            let (s, e) = read_str(b, i + 1, c);
            t.push(Tok::Str(s));
            i = e;
        } else if c == b'`' {
            let (s, e, expr) = read_tpl(b, i + 1);
            t.push(if expr { Tok::Opaque } else { Tok::Str(s) });
            i = e;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_' || ((b[i] == b'-' || b[i] == b'+') && matches!(b[i - 1], b'e' | b'E') && !src[st..i].starts_with("0x"))) {
                i += 1;
            }
            let s = src[st..i].replace('_', "");
            let n = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) { i64::from_str_radix(h, 16).ok().map(|v| v as f64) } else { s.parse::<f64>().ok() };
            t.push(n.map_or(Tok::Opaque, Tok::Num));
        } else if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 {
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 0x80) {
                i += 1;
            }
            t.push(Tok::Ident(src[st..i].to_owned()));
        } else {
            t.push(Tok::P(c));
            i += 1;
        }
    }
    t
}

fn regex_allowed(prev: Option<&Tok>) -> bool {
    match prev {
        None => true,
        Some(Tok::P(c)) => b"(,=:[!&|?{};+-*%<>~^".contains(c),
        Some(Tok::Ident(w)) => matches!(w.as_str(), "return" | "typeof" | "case" | "do" | "else" | "in" | "of" | "new" | "delete" | "void" | "throw"),
        _ => false,
    }
}

/// End of a `/regex/flags` starting at `i`, or None if the line ends first.
fn skip_regex(b: &[u8], mut i: usize) -> Option<usize> {
    i += 1;
    let mut class = false;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'\n' => return None,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => {
                i += 1;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Push the escape at `b[i]` (after the backslash); returns the index after it.
fn read_escape(b: &[u8], i: usize, out: &mut Vec<u8>) -> usize {
    let Some(&e) = b.get(i) else { return i };
    let mut push = |c: char| out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
    let hex = |s: usize, n: usize| std::str::from_utf8(b.get(s..s + n)?).ok().and_then(|h| u32::from_str_radix(h, 16).ok());
    match e {
        b'n' => push('\n'),
        b't' => push('\t'),
        b'r' => push('\r'),
        b'0' => push('\0'),
        b'\n' => {}
        b'x' => {
            if let Some(c) = hex(i + 1, 2).and_then(char::from_u32) {
                push(c);
                return i + 3;
            }
        }
        b'u' if b.get(i + 1) == Some(&b'{') => {
            if let Some(end) = b[i..].iter().position(|&c| c == b'}') {
                if let Some(c) = hex(i + 2, end - 2).and_then(char::from_u32) {
                    push(c);
                }
                return i + end + 1;
            }
        }
        b'u' => {
            if let Some(mut u) = hex(i + 1, 4) {
                let mut n = i + 5;
                // Surrogate pair (emoji in category names).
                if (0xD800..0xDC00).contains(&u) && b.get(n) == Some(&b'\\') && b.get(n + 1) == Some(&b'u') {
                    if let Some(lo) = hex(n + 2, 4) {
                        u = 0x10000 + ((u - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                        n += 6;
                    }
                }
                push(char::from_u32(u).unwrap_or('\u{FFFD}'));
                return n;
            }
        }
        _ => out.push(e),
    }
    i + 1
}

fn read_str(b: &[u8], mut i: usize, q: u8) -> (String, usize) {
    let mut out = Vec::new();
    while i < b.len() && b[i] != q && b[i] != b'\n' {
        if b[i] == b'\\' {
            i = read_escape(b, i + 1, &mut out);
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    (String::from_utf8_lossy(&out).into_owned(), i + 1)
}

/// Template literal body from `i`; `(text, end, had ${…})`.
fn read_tpl(b: &[u8], mut i: usize) -> (String, usize, bool) {
    let (mut out, mut expr) = (Vec::new(), false);
    while i < b.len() && b[i] != b'`' {
        if b[i] == b'\\' {
            i = read_escape(b, i + 1, &mut out);
        } else if b[i] == b'$' && b.get(i + 1) == Some(&b'{') {
            expr = true;
            i += 2;
            let mut depth = 1;
            while i < b.len() && depth > 0 {
                match b[i] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    b'"' | b'\'' => i = read_str(b, i + 1, b[i]).1 - 1,
                    b'`' => i = read_tpl(b, i + 1).1 - 1,
                    _ => {}
                }
                i += 1;
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    (String::from_utf8_lossy(&out).into_owned(), i + 1, expr)
}

struct Js {
    t: Vec<Tok>,
    /// Index of the matching bracket, `usize::MAX` if unmatched.
    mate: Vec<usize>,
    /// `const|let|var X = literal` (and `X.key` for object literals); None = declared twice.
    consts: HashMap<String, Option<Value>>,
}

/// Well-known enum members whose owners are imported (so unreadable as local consts).
/// Values from the frontend's `constants/globalEnums.ts` (LiteGraph's LinkRenderType).
fn enum_member(name: &str) -> Option<i64> {
    Some(match name {
        "HIDDEN_LINK" => -1,
        "STRAIGHT_LINK" => 0,
        "LINEAR_LINK" => 1,
        "SPLINE_LINK" => 2,
        _ => return None,
    })
}

impl Js {
    fn new(src: &str) -> Self {
        let t = lex(src);
        let mut mate = vec![usize::MAX; t.len()];
        let mut stack: Vec<(u8, usize)> = Vec::new();
        for (i, tok) in t.iter().enumerate() {
            let Tok::P(c) = tok else { continue };
            let open = match c {
                b')' => b'(',
                b']' => b'[',
                b'}' => b'{',
                b'(' | b'[' | b'{' => {
                    stack.push((*c, i));
                    continue;
                }
                _ => continue,
            };
            if let Some(k) = stack.iter().rposition(|(o, _)| *o == open) {
                let (_, j) = stack[k];
                stack.truncate(k);
                mate[j] = i;
                mate[i] = j;
            }
        }
        let mut js = Js { t, mate, consts: HashMap::new() };
        js.consts = js.read_consts();
        js
    }

    fn p(&self, i: usize, c: u8) -> bool {
        self.t.get(i) == Some(&Tok::P(c))
    }

    fn ident(&self, i: usize) -> Option<&str> {
        match self.t.get(i) {
            Some(Tok::Ident(s)) => Some(s),
            _ => None,
        }
    }

    /// Past the expression at `i`: stops at a `,` or closer at depth 0, or `end`.
    fn skip_expr(&self, mut i: usize, end: usize) -> usize {
        while i < end {
            match self.t[i] {
                Tok::P(b'(' | b'[' | b'{') if self.mate[i] != usize::MAX => i = self.mate[i] + 1,
                Tok::P(b',' | b')' | b']' | b'}' | b';') => break,
                _ => i += 1,
            }
        }
        i.min(end)
    }

    /// A literal at `i` → (value, index after it). The caller checks what follows.
    fn literal(&self, i: usize) -> (Option<Value>, usize) {
        let num = |n: f64| serde_json::Number::from_f64(n).map(|x| if n.fract() == 0.0 && n.abs() < 9e15 { Value::from(n as i64) } else { Value::Number(x) });
        match self.t.get(i) {
            Some(Tok::Str(s)) => (Some(Value::String(s.clone())), i + 1),
            Some(Tok::Num(n)) => (num(*n), i + 1),
            Some(Tok::P(b'-')) => match self.t.get(i + 1) {
                Some(Tok::Num(n)) => (num(-n), i + 2),
                _ => (None, i + 1),
            },
            Some(Tok::P(b'!')) => match self.t.get(i + 1) {
                Some(Tok::Num(n)) => (Some(Value::Bool(*n == 0.0)), i + 2),
                _ => (None, i + 1),
            },
            Some(Tok::P(b'[')) if self.mate[i] != usize::MAX => {
                let end = self.mate[i];
                let (mut j, mut arr, mut ok) = (i + 1, Vec::new(), true);
                while j < end {
                    if self.p(j, b',') {
                        j += 1;
                        continue;
                    }
                    let (v, n) = self.literal(j);
                    match v {
                        Some(v) if n == end || self.p(n, b',') => arr.push(v),
                        _ => ok = false,
                    }
                    j = self.skip_expr(n.max(j + 1), end);
                }
                // `["a", "b"].map(v => ({text: v, value: v}))` (EreNodes): the labels are the values.
                if ok && self.p(end + 1, b'.') && self.ident(end + 2) == Some("map") && self.p(end + 3, b'(') && self.mate[end + 3] != usize::MAX {
                    return (Some(Value::Array(arr)), self.mate[end + 3] + 1);
                }
                (ok.then_some(Value::Array(arr)), end + 1)
            }
            Some(Tok::P(b'{')) if self.mate[i] != usize::MAX => {
                let o: Map<String, Value> = self.props(i).into_iter().filter_map(|(k, v)| Some((k, v?))).collect();
                (Some(Value::Object(o)), self.mate[i] + 1)
            }
            Some(Tok::Ident(w)) => match w.as_str() {
                "true" => (Some(Value::Bool(true)), i + 1),
                "false" => (Some(Value::Bool(false)), i + 1),
                "null" => (Some(Value::Null), i + 1),
                _ => {
                    // `X` or `X.key`, resolved through the file's constants.
                    let (mut name, mut j) = (w.clone(), i + 1);
                    while self.p(j, b'.') {
                        let Some(k) = self.ident(j + 1) else { break };
                        name = format!("{name}.{k}");
                        j += 2;
                    }
                    // Imported litegraph enums (`Y.STRAIGHT_LINK`) have no local const to read.
                    let v = self.consts.get(&name).cloned().flatten().or_else(|| {
                        let member = name.rsplit('.').next().unwrap_or(&name);
                        enum_member(member).map(Value::from)
                    });
                    (v, j)
                }
            },
            _ => (None, i + 1),
        }
    }

    /// The `key: value` pairs directly inside the `{` at `open`; non-literal values are None.
    fn props(&self, open: usize) -> Vec<(String, Option<Value>)> {
        let end = self.mate[open];
        let mut out = Vec::new();
        let mut i = open + 1;
        while i < end {
            let key = match &self.t[i] {
                Tok::Ident(s) | Tok::Str(s) => s.clone(),
                Tok::Num(n) => n.to_string(),
                Tok::P(b',') => {
                    i += 1;
                    continue;
                }
                _ => {
                    i = self.skip_expr(i + 1, end);
                    continue;
                }
            };
            if self.p(i + 1, b':') {
                let (v, n) = self.literal(i + 2);
                let lit = n >= end || self.p(n, b',');
                out.push((key, if lit { v } else { None }));
                i = self.skip_expr(n.max(i + 3), end);
            } else if self.p(i + 1, b',') || i + 1 == end {
                // Shorthand `{ id, name }`.
                let v = self.consts.get(&key).cloned().flatten();
                out.push((key, v));
                i += 1;
            } else {
                // Method (`onChange(v) {…}`, `async onChange…`) or not an object at all.
                if self.p(i + 1, b'(') || self.ident(i + 1).is_some() {
                    out.push((self.ident(i + 1).filter(|_| matches!(key.as_str(), "async" | "get" | "set")).unwrap_or(&key).to_owned(), None));
                }
                i = self.skip_expr(i + 1, end);
            }
        }
        out
    }

    fn read_consts(&self) -> HashMap<String, Option<Value>> {
        let mut m: HashMap<String, Option<Value>> = HashMap::new();
        let mut put = |k: String, v: Option<Value>| {
            m.entry(k).and_modify(|e| *e = None).or_insert(v);
        };
        let mut i = 0;
        while i < self.t.len() {
            if !matches!(self.ident(i), Some("const" | "let" | "var")) {
                i += 1;
                continue;
            }
            i += 1;
            while let Some(name) = self.ident(i) {
                let name = name.to_owned();
                if !self.p(i + 1, b'=') {
                    i += 1;
                    break;
                }
                let (v, n) = self.literal(i + 2);
                let done = matches!(self.t.get(n), None | Some(Tok::P(b';' | b'}')) | Some(Tok::Ident(_)));
                let v = if done || self.p(n, b',') { v } else { None };
                if let Some(Value::Object(o)) = &v {
                    for (k, x) in o {
                        put(format!("{name}.{k}"), Some(x.clone()));
                    }
                }
                put(name, v);
                i = self.skip_expr(n.max(i + 3), self.t.len());
                if !self.p(i, b',') {
                    break;
                }
                i += 1;
            }
        }
        m
    }

    /// Setting definitions and `registerExtension({name})` names in this file.
    fn extract(&self) -> (Vec<SettingDef>, Vec<String>) {
        let (mut defs, mut names) = (Vec::new(), Vec::new());
        for i in 0..self.t.len() {
            if self.ident(i) == Some("registerExtension") && self.p(i + 1, b'(') && self.p(i + 2, b'{') && self.mate[i + 2] != usize::MAX {
                if let Some((_, Some(Value::String(n)))) = self.props(i + 2).into_iter().find(|(k, _)| k == "name") {
                    names.push(n);
                }
            }
            if !self.p(i, b'{') || self.mate[i] == usize::MAX {
                continue;
            }
            let props: HashMap<String, Option<Value>> = self.props(i).into_iter().collect();
            if let Some(d) = to_def(&props) {
                defs.push(d);
            }
        }
        (defs, names)
    }
}

fn to_def(p: &HashMap<String, Option<Value>>) -> Option<SettingDef> {
    let Some(Some(Value::String(id))) = p.get("id") else { return None };
    if !p.contains_key("type") || !(p.contains_key("name") || p.contains_key("defaultValue")) || id.is_empty() {
        return None;
    }
    let s = |k: &str| p.get(k).cloned().flatten().and_then(|v| v.as_str().map(str::to_owned));
    let path = match p.get("category").cloned().flatten() {
        Some(Value::Array(a)) if !a.is_empty() && a.iter().all(Value::is_string) => a.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_owned())).collect(),
        _ => id.split('.').map(str::to_owned).collect(),
    };
    // `options: {key: label}` (the i18n bundles) keeps the key as the stored value.
    let options = match p.get("options").cloned().flatten() {
        Some(Value::Object(m)) => m
            .into_iter()
            .map(|(k, v)| (v.as_str().map(str::to_owned).unwrap_or_else(|| k.clone()), Value::String(k)))
            .collect(),
        Some(Value::Array(a)) => a
            .into_iter()
            .filter_map(|o| match o {
                Value::Object(m) => {
                    let v = m.get("value").cloned()?;
                    let label = m.get("text").or(m.get("label")).and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| plain(&v));
                    Some((label, v))
                }
                v => Some((plain(&v), v)),
            })
            .collect(),
        _ => Vec::new(),
    };
    let attr = |k: &str| p.get("attrs").cloned().flatten().and_then(|a| a.get(k).and_then(Value::as_f64));
    Some(SettingDef {
        id: id.clone(),
        name: s("name").unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).to_owned()),
        path,
        tooltip: s("tooltip").unwrap_or_default(),
        kind: s("type").unwrap_or_else(|| "custom".into()),
        options,
        default: p.get("defaultValue").cloned().flatten().unwrap_or(Value::Null),
        min: attr("min"),
        max: attr("max"),
        step: attr("step"),
    })
}

/// A JSON value as display text (strings unquoted).
pub fn plain(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(src: &str) -> (Vec<SettingDef>, Vec<String>) {
        Js::new(src).extract()
    }

    #[test]
    fn minified_backticks() {
        let (d, _) = scan("fS=[{id:`Comfy.NodeSearchBoxImpl.NodePreview`,category:[`Comfy`,`Node Search Box`,`NodePreview`],name:`Node preview`,tooltip:`Show a ${x} preview`,type:`boolean`,defaultValue:!0},{id:`Comfy.Validation.Workflows`,name:`Validate workflows`,type:`boolean`,defaultValue:!1,onChange:e=>{if(/[`}]/.test(e))return}},{id:`max-upload-size`,name:`Max`,category:[`Network`],type:`number`,defaultValue:100}]");
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].path, ["Comfy", "Node Search Box", "NodePreview"]);
        assert_eq!(d[0].name, "Node preview");
        assert_eq!(d[0].tooltip, "", "templates with ${{}} are not literals");
        assert_eq!(d[0].default, Value::Bool(true));
        assert_eq!((d[1].category(), d[1].section(), &d[1].default), ("Comfy", "Validation", &Value::Bool(false)));
        assert_eq!(d[2].category(), "Other");
    }

    #[test]
    fn combo_options_resolve_imported_enums() {
        let (d, _) = scan("{id:`Comfy.LinkRenderMode`,category:[`LiteGraph`,`Graph`,`LinkRenderMode`],name:`Link Render Mode`,defaultValue:2,type:`combo`,options:[{value:Y.STRAIGHT_LINK,text:`Straight`},{value:Y.LINEAR_LINK,text:`Linear`},{value:Y.SPLINE_LINK,text:`Spline`},{value:Y.HIDDEN_LINK,text:`Hidden`}]}");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].options, [("Straight".to_owned(), Value::from(0)), ("Linear".to_owned(), Value::from(1)), ("Spline".to_owned(), Value::from(2)), ("Hidden".to_owned(), Value::from(-1))]);
    }

    #[test]
    fn settings_array_and_consts() {
        let src = r#"
const SETTING_CATEGORY_NAME = "Image Saver";
const SECTION = 'File Handling', WHEEL_ID = "LoraManager.Wheel";
const ID = { tool: "LC123.Align.Enabled" };
app.registerExtension({
    name: "ComfyUI-Image-Saver",
    settings: [
        {
            id: "ImageSaver.HandleImageWorkflowDrop",
            name: "Use a custom file drop handler", // comment with "quote
            type: "boolean",
            defaultValue: true,
            category: [SETTING_CATEGORY_NAME, SECTION, "Custom File Drop Handler"],
            tooltip: "Line one.\n" + "Line two",
            async onChange(value) { await fetch('/x', {method: "POST"}); },
        },
        { id: WHEEL_ID, name: "Wheel", type: "slider", attrs: { min: 0.01, max: 0.1, step: 0.01 }, defaultValue: -2,
          category: ["LoRA Manager", "Trigger Word Toggle", "Wheel"] },
        { id: ID.tool, name: "Align tool", type: "combo", options: [{ text: "On", value: 1 }, "Off"], defaultValue: 1 },
        { id: "VHS.Previews", type: 'combo', options: ['Never', 'Always'].map(v => ({text: v, value: v})), defaultValue: 'Never', category: ['🎥\uD83C\uDD65', 'Previews', 'Advanced'] },
        { id: "rgthree.config", name: "Open config", defaultValue: null, type: () => document.createElement("div") },
    ],
});
"#;
        let (d, names) = scan(src);
        assert_eq!(names, ["ComfyUI-Image-Saver"]);
        assert_eq!(d.len(), 5, "{d:#?}");
        assert_eq!(d[0].path, ["Image Saver", "File Handling", "Custom File Drop Handler"]);
        assert_eq!(d[0].tooltip, "", "concatenation is not a literal");
        assert_eq!((d[1].id.as_str(), d[1].min, d[1].step, &d[1].default), ("LoraManager.Wheel", Some(0.01), Some(0.01), &Value::from(-2)));
        assert_eq!(d[2].id, "LC123.Align.Enabled");
        assert_eq!(d[2].options, [("On".to_owned(), Value::from(1)), ("Off".to_owned(), Value::from("Off"))]);
        assert_eq!(d[3].options.len(), 2);
        assert_eq!(d[3].category(), "🎥🅥");
        assert_eq!(d[4].kind, "custom");
    }

    #[test]
    fn add_setting_calls() {
        let (d, _) = scan("for(let k in Ve)dt=Ve[k],ce.ui.settings.addSetting(dt);const Ve={a:{id:\"EasyUse.Hotkeys.AddGroup\",name:\"Enable Shift+g\",type:\"boolean\",defaultValue:!0}};\napp.ui.settings.addSetting({ id: 'EreNodes.Autocomplete.Global', name: 'Global', type: 'boolean', defaultValue: true, onChange: (v) => { fetch(\"/x\") } });");
        let ids: Vec<&str> = d.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["EasyUse.Hotkeys.AddGroup", "EreNodes.Autocomplete.Global"]);
        assert_eq!(d[0].path, ["EasyUse", "Hotkeys", "AddGroup"]);
    }

    /// `cargo test settings_defs -- --ignored --nocapture` against the local backend.
    #[test]
    #[ignore]
    fn live_categories() {
        let c = Client::new("127.0.0.1", std::env::var("COMFY_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8188));
        let t = std::time::Instant::now();
        let cat = catalog(&c);
        let values = c.settings().unwrap_or_default();
        let view = merge(&cat, &values);
        println!("{} defs, {} hidden, {} extensions in {:?}", cat.defs.len(), cat.hidden.len(), cat.extensions.len(), t.elapsed());
        for c in &view.categories {
            let n = view.defs.iter().filter(|d| d.category() == c).count();
            let syn = view.defs[cat.defs.len()..].iter().filter(|d| d.category() == c).count();
            println!("  {:<20} {n:>3} settings ({syn} from /api/settings only)", category_label(c));
        }
        for want in ["Comfy", "LiteGraph", "Appearance", "KJNodes", "EasyUse", "LoRA Manager", "pysssss", "rgthree", "EreNodes"] {
            assert!(view.categories.iter().any(|c| c == want), "missing {want}");
        }
    }
}
