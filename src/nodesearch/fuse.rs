// SPDX-License-Identifier: GPL-3.0-only
//! Port of the parts of Fuse.js 7.0.0 the ComfyUI frontend search uses
//! (`fuse.js/src/search/{bitap,extended}`, `core/computeScore.js`, `tools/norm.js`).
//! Options are the frontend's: `threshold` per call, `location 0`, `distance 100`,
//! `ignoreLocation false`, `findAllMatches false`, `minMatchCharLength 1`, case-insensitive.
//! Kept free of any app types so a Fuse upgrade only touches this file.

const MAX_BITS: usize = 32;
const DISTANCE: f64 = 100.0;

/// `computeScore` (location-aware).
fn compute_score(pat_len: usize, errors: usize, cur: i64, expected: i64) -> f64 {
    errors as f64 / pat_len as f64 + (expected - cur).abs() as f64 / DISTANCE
}

/// One bitap chunk (≤ 32 chars): `search()` from `bitap/search.js`. Returns (matched, score).
fn bitap_chunk(text: &[char], pat: &[char], alphabet: &std::collections::HashMap<char, u32>, location: usize, threshold: f64) -> (bool, f64) {
    let (plen, tlen) = (pat.len(), text.len());
    let expected = location.min(tlen) as i64;
    let mut cur_threshold = threshold;
    // Exact occurrences lower the threshold first.
    let mut from = expected as usize;
    while let Some(index) = find(text, pat, from) {
        let score = compute_score(plen, 0, index as i64, expected);
        cur_threshold = cur_threshold.min(score);
        from = index + plen;
    }
    let mut best_loc: i64 = -1;
    let mut last: Vec<u32> = vec![];
    let mut final_score = 1.0f64;
    let mut bin_max = (plen + tlen) as i64;
    let mask: u32 = 1u32 << (plen - 1);
    for i in 0..plen {
        let (mut bin_min, mut bin_mid) = (0i64, bin_max);
        while bin_min < bin_mid {
            let score = compute_score(plen, i, expected + bin_mid, expected);
            if score <= cur_threshold {
                bin_min = bin_mid;
            } else {
                bin_max = bin_mid;
            }
            bin_mid = (bin_max - bin_min) / 2 + bin_min;
        }
        bin_max = bin_mid;
        let mut start = 1.max(expected - bin_mid + 1);
        let finish = ((expected + bin_mid).min(tlen as i64) + plen as i64) as usize;
        let mut bits = vec![0u32; finish + 2];
        bits[finish + 1] = (1u32 << i).wrapping_sub(1);
        let mut j = finish as i64;
        while j >= start {
            let cl = (j - 1) as usize;
            let char_match = text.get(cl).and_then(|c| alphabet.get(c)).copied().unwrap_or(0);
            let ju = j as usize;
            bits[ju] = ((bits[ju + 1] << 1) | 1) & char_match;
            if i > 0 {
                let (l1, l0) = (last.get(ju + 1).copied().unwrap_or(0), last.get(ju).copied().unwrap_or(0));
                bits[ju] |= ((l1 | l0) << 1) | 1 | l1;
            }
            if bits[ju] & mask != 0 {
                final_score = compute_score(plen, i, cl as i64, expected);
                if final_score <= cur_threshold {
                    cur_threshold = final_score;
                    best_loc = cl as i64;
                    if best_loc <= expected {
                        break;
                    }
                    start = 1.max(2 * expected - best_loc);
                }
            }
            j -= 1;
        }
        if compute_score(plen, i + 1, expected, expected) > cur_threshold {
            break;
        }
        last = bits;
    }
    (best_loc >= 0, final_score.max(0.001))
}

fn find(text: &[char], pat: &[char], from: usize) -> Option<usize> {
    if pat.is_empty() || from > text.len() || text.len() < pat.len() {
        return None;
    }
    (from..=text.len() - pat.len()).find(|&i| text[i..i + pat.len()] == *pat)
}

/// `BitapSearch`: a fuzzy pattern, chunked at 32 chars.
pub struct Bitap {
    pattern: Vec<char>,
    chunks: Vec<(Vec<char>, std::collections::HashMap<char, u32>, usize)>,
    threshold: f64,
}

impl Bitap {
    pub fn new(pattern: &str, threshold: f64) -> Self {
        let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
        let mut chunks = vec![];
        let mut add = |p: &[char], start: usize| {
            let mut a = std::collections::HashMap::new();
            for (i, c) in p.iter().enumerate() {
                *a.entry(*c).or_insert(0u32) |= 1u32 << (p.len() - i - 1);
            }
            chunks.push((p.to_vec(), a, start));
        };
        let len = pattern.len();
        if len > MAX_BITS {
            let end = len - len % MAX_BITS;
            let mut i = 0;
            while i < end {
                add(&pattern[i..i + MAX_BITS], i);
                i += MAX_BITS;
            }
            if len % MAX_BITS != 0 {
                add(&pattern[len - MAX_BITS..], len - MAX_BITS);
            }
        } else if len > 0 {
            add(&pattern, 0);
        }
        Self { pattern, chunks, threshold }
    }

    /// `searchIn`: Some(score) when matched. `text` is lowercased here.
    pub fn search_in(&self, text: &str) -> Option<f64> {
        let text: Vec<char> = text.to_lowercase().chars().collect();
        if self.pattern == text {
            return Some(0.0);
        }
        if self.chunks.is_empty() {
            return None;
        }
        let (mut any, mut total) = (false, 0.0);
        for (p, a, start) in &self.chunks {
            let (m, s) = bitap_chunk(&text, p, a, *start, self.threshold);
            any |= m;
            total += s;
        }
        any.then(|| total / self.chunks.len() as f64)
    }
}

/// Extended-search token kinds (`search/extended/*Match.js`).
enum Tok {
    Exact(String),
    Include(String),
    Prefix(String),
    InvPrefix(String),
    InvSuffix(String),
    Suffix(String),
    InvExact(String),
    Fuzzy(Bitap),
}

/// `(prefix, suffix, quoted-prefix…)` regexes of the searchers, tried in Fuse's order:
/// all multi (quoted) forms first, then all single forms.
fn parse_token(item: &str, threshold: f64) -> Option<Tok> {
    let quoted = |pre: &str, post: &str| item.strip_prefix(pre).and_then(|r| r.strip_suffix(post)).map(str::to_owned);
    let rest = |pre: &str, post: &str| item.strip_prefix(pre).and_then(|r| if post.is_empty() { Some(r) } else { r.strip_suffix(post) }).map(str::to_owned);
    // multi
    if let Some(t) = quoted("=\"", "\"") { return Some(Tok::Exact(t)) }
    if let Some(t) = quoted("'\"", "\"") { return Some(Tok::Include(t)) }
    if let Some(t) = quoted("^\"", "\"") { return Some(Tok::Prefix(t)) }
    if let Some(t) = quoted("!^\"", "\"") { return Some(Tok::InvPrefix(t)) }
    if let Some(t) = quoted("!\"", "$\"") { return Some(Tok::InvSuffix(t)) }
    if let Some(t) = quoted("\"", "$\"") { return Some(Tok::Suffix(t)) }
    if let Some(t) = quoted("!\"", "\"") { return Some(Tok::InvExact(t)) }
    if let Some(t) = quoted("\"", "\"") { return Some(Tok::Fuzzy(Bitap::new(&t, threshold))) }
    // single
    if let Some(t) = rest("=", "") { return Some(Tok::Exact(t)) }
    if let Some(t) = rest("'", "") { return Some(Tok::Include(t)) }
    if let Some(t) = rest("^", "") { return Some(Tok::Prefix(t)) }
    if let Some(t) = rest("!^", "") { return Some(Tok::InvPrefix(t)) }
    if let Some(t) = rest("!", "$") { return Some(Tok::InvSuffix(t)) }
    if let Some(t) = rest("", "$") { return Some(Tok::Suffix(t)) }
    if let Some(t) = rest("!", "") { return Some(Tok::InvExact(t)) }
    Some(Tok::Fuzzy(Bitap::new(item, threshold)))
}

impl Tok {
    fn search(&self, text: &str) -> Option<f64> {
        let hit = |b: bool| b.then_some(0.0);
        match self {
            Tok::Exact(p) => hit(text == p.to_lowercase()),
            Tok::Include(p) => hit(text.contains(&p.to_lowercase())),
            Tok::Prefix(p) => hit(text.starts_with(&p.to_lowercase())),
            Tok::InvPrefix(p) => hit(!text.starts_with(&p.to_lowercase())),
            Tok::InvSuffix(p) => hit(!text.ends_with(&p.to_lowercase())),
            Tok::Suffix(p) => hit(text.ends_with(&p.to_lowercase())),
            Tok::InvExact(p) => hit(!text.contains(&p.to_lowercase())),
            Tok::Fuzzy(b) => b.search_in(text),
        }
    }
}

/// Fuse `ExtendedSearch`: `|` separates OR groups, spaces (outside quotes) AND tokens.
pub struct Extended(Vec<Vec<Tok>>);

/// `SPACE_RE = / +(?=(?:[^"]*"[^"]*")*[^"]*$)/`: split on spaces outside quotes.
fn split_spaces(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (vec![], String::new(), false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ' ' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().filter(|t| !t.trim().is_empty()).collect()
}

impl Extended {
    pub fn new(query: &str, threshold: f64) -> Self {
        Self(
            query
                .split('|')
                .map(|g| split_spaces(g.trim()).iter().filter_map(|t| parse_token(t, threshold)).collect())
                .collect(),
        )
    }

    /// `searchIn`: the first OR group whose tokens all match decides; score = mean token score.
    pub fn search_in(&self, text: &str) -> Option<f64> {
        let text = text.to_lowercase();
        'groups: for g in &self.0 {
            let (mut n, mut total) = (0, 0.0);
            for t in g {
                match t.search(&text) {
                    Some(s) => {
                        n += 1;
                        total += s;
                    }
                    None => continue 'groups,
                }
            }
            if n > 0 {
                return Some(total / n as f64);
            }
        }
        None
    }
}

/// `tools/norm.js` with `fieldNormWeight 1`, 3 decimals: 1/√(number of space-separated tokens).
fn norm(text: &str) -> f64 {
    let n = text.split(' ').filter(|t| !t.is_empty()).count().max(1) as f64;
    ((1.0 / n.sqrt()) * 1000.0).round() / 1000.0
}

/// `_searchObjectList` + `computeScore` (`core/computeScore.js`): `keys` holds, per key, the
/// string values (one for string keys, several for array keys). Keys are equally weighted.
/// Returns Some(score) (0 = best) when any key matched.
pub fn score_object(searcher: &Extended, keys: &[Vec<&str>]) -> Option<f64> {
    let weight = 1.0 / keys.len() as f64;
    let (mut any, mut total) = (false, 1.0f64);
    for v in keys.iter().flatten().filter(|v| !v.trim().is_empty()) {
        if let Some(s) = searcher.search_in(v) {
            any = true;
            total *= (if s == 0.0 { f64::EPSILON } else { s }).powf(weight * norm(v));
        }
    }
    any.then_some(total)
}

/// Plain (non-extended) Fuse search over a string list, as the type-filter popover does:
/// `new Fuse(options, {includeScore, threshold: .3, shouldSort: true}).search(q)`.
pub fn search_strings<'a>(items: &'a [String], query: &str, threshold: f64) -> Vec<&'a String> {
    let b = Bitap::new(query, threshold);
    let mut hits: Vec<(f64, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, t)| b.search_in(t).map(|s| ((if s == 0.0 { 1.0 } else { s }).powf(norm(t)), i)))
        .collect();
    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, i)| &items[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(q: &str, t: &str) -> bool {
        Extended::new(q, 0.3).search_in(t).is_some()
    }

    #[test]
    fn fuzzy_threshold() {
        assert!(m("ksampler", "KSampler"));
        assert!(m("ksampelr", "KSampler")); // transposition: 2 errors on 8 chars is within .3
        assert!(m("sampler", "KSampler Advanced"));
        assert!(!m("zzzzzz", "KSampler"));
        assert_eq!(Extended::new("ksampler", 0.3).search_in("KSampler"), Some(0.0));
    }

    #[test]
    fn extended_operators() {
        assert!(m("^ks", "KSampler") && !m("^sam", "KSampler"));
        assert!(m("=ksampler", "KSampler") && !m("=ksam", "KSampler"));
        assert!(m("'amp", "KSampler") && !m("!amp", "KSampler"));
        assert!(m("vae|clip", "CLIPTextEncode") && !m("vae clip", "CLIPTextEncode"));
        assert!(m("clip text", "CLIPTextEncode") && m("\"text enc\"", "clip text encode"));
    }

    #[test]
    fn long_pattern_is_chunked() {
        let t = "load checkpoint with config (deprecated) for the old unet";
        // 57 chars > 32 bits: matched in 32-bit chunks (a full chunk at 0, the remainder at len-32).
        assert_eq!(Bitap::new(t, 0.3).search_in(t), Some(0.0));
        assert!(Bitap::new(t, 0.3).search_in(&format!("{t} x")).is_some());
        assert!(Bitap::new(t, 0.3).search_in("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_none());
    }
}
