// SPDX-License-Identifier: GPL-3.0-only
//! Port of `src/utils/fuseUtil.ts` (frontend 1.60.0): the "advanced scoring" re-rank that
//! sits on top of Fuse and `FuseFilter.matches`. Generic over the entry via `Entry`.

pub type Aux = Vec<f64>;

/// What `FuseSearch` needs from an item: its string keys (name, display_name) for scoring
/// and the `postProcessSearchScores` hook.
pub trait Entry {
    fn values(&self) -> Vec<&str>;
    fn post_process(&self, scores: Aux) -> Aux;
}

/// JS `\b` + camelCase split used for `itemWords`; spaces are separators and dropped.
fn words(item: &str) -> Vec<String> {
    let c: Vec<char> = item.chars().collect();
    let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let boundary = |q: usize| {
        let (a, b) = (c[q - 1], c[q]);
        word(a) != word(b)
            || (a.is_ascii_lowercase() && b.is_ascii_uppercase())
            || (b.is_ascii_uppercase() && c.get(q + 1).is_some_and(|n| n.is_ascii_lowercase()))
    };
    let (mut out, mut p) = (vec![], 0);
    for q in 0..c.len() {
        if c[q] == ' ' {
            out.push(c[p..q].iter().collect::<String>());
            p = q + 1;
        } else if q > p && boundary(q) {
            out.push(c[p..q].iter().collect::<String>());
            p = q;
        }
    }
    out.push(c[p..].iter().collect::<String>());
    out.into_iter().map(|w| w.to_lowercase()).collect()
}

fn char_len(s: &str) -> f64 {
    s.chars().count() as f64
}

/// JS `indexOf` in UTF-16 units; chars are close enough for node names.
fn index_of(hay: &str, needle: &str) -> f64 {
    hay.find(needle).map_or(-1.0, |b| hay[..b].chars().count() as f64)
}

/// `calcAuxSingle(query, item, score)`.
pub fn calc_aux_single(query: &str, item: &str, score: f64) -> Aux {
    let item_words = words(item);
    let parts: Vec<&str> = query.split(' ').collect();
    let item = item.to_lowercase();
    let len = char_len(&item);
    let (mut main, mut aux1, mut aux2) = (9.0, 0.0, 0.0);
    if item == query {
        main = 0.0;
    } else if item.starts_with(query) {
        main = 1.0;
        aux2 = len;
    } else if item_words.iter().any(|w| w == query) {
        main = 2.0;
        aux1 = index_of(&item, query) + len * 0.5;
        aux2 = len;
    } else if item.contains(query) {
        main = 3.0;
        aux1 = index_of(&item, query) + len * 0.5;
        aux2 = len;
    } else if parts.iter().all(|p| item_words.iter().any(|w| w == p)) {
        let idx: Vec<f64> = parts.iter().map(|p| item_words.iter().position(|w| w == p).unwrap() as f64).collect();
        let (min, max) = (idx.iter().cloned().fold(f64::MAX, f64::min), idx.iter().cloned().fold(f64::MIN, f64::max));
        main = 4.0;
        aux1 = max - min + max * 0.5 + len * 0.5;
        aux2 = len;
    } else if parts.iter().all(|p| item.contains(p)) {
        let min = parts.iter().map(|p| index_of(&item, p)).fold(f64::MAX, f64::min);
        let max = parts.iter().map(|p| index_of(&item, p) + char_len(p)).fold(f64::MIN, f64::max);
        main = 5.0;
        aux1 = max - min + max * 0.5 + len * 0.5;
        aux2 = len;
    }
    let (a, b) = (len.min(char_len(query)), len.max(char_len(query)));
    vec![main, aux1, aux2, score + 0.2 * (1.0 - a / b)]
}

/// `compareAux`: lexicographic.
pub fn compare_aux(a: &Aux, b: &Aux) -> std::cmp::Ordering {
    for (x, y) in a.iter().zip(b) {
        if x != y {
            return x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal);
        }
    }
    a.len().cmp(&b.len())
}

/// `calcAuxScores(query, entry, score)` (query already lowercased).
pub fn calc_aux_scores<E: Entry>(query: &str, entry: &E, score: f64) -> Aux {
    let values = entry.values();
    let mut scores: Vec<Aux> = values.iter().map(|v| calc_aux_single(query, v, score)).collect();
    scores.sort_by(compare_aux);
    let mut result = scores.swap_remove(0);
    if values.iter().any(|v| v.to_lowercase().contains("deprecated")) && result[0] != 0.0 {
        result[0] += 5.0;
    }
    entry.post_process(result)
}

/// `FuseFilter.matches`: does the item's option list satisfy `value` ("A,B" = any of)?
/// With `wildcard`, only items that expose that wildcard option match.
pub fn filter_matches(options: &[String], value: &str, wildcard: Option<&str>) -> bool {
    if let Some(w) = wildcard {
        return value == w || options.iter().any(|o| o == w);
    }
    if !value.contains(',') {
        return options.iter().any(|o| o == value);
    }
    value.split(',').any(|v| options.iter().any(|o| o == v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_split_matches_js() {
        assert_eq!(words("KSamplerAdvanced"), ["k", "sampler", "advanced"]);
        assert_eq!(words("Load Image (Mask)"), ["load", "image", "(", "mask", ")"]);
    }

    #[test]
    fn aux_ranking() {
        assert_eq!(calc_aux_single("ksampler", "KSampler", 0.0)[0], 0.0);
        assert_eq!(calc_aux_single("k", "KSampler", 0.0)[0], 1.0);
        assert_eq!(calc_aux_single("sampler", "KSampler", 0.0)[0], 2.0);
        assert_eq!(calc_aux_single("ampl", "KSampler", 0.0)[0], 3.0);
    }

    #[test]
    fn filter() {
        let o = vec!["IMAGE".to_string(), "*".to_string()];
        assert!(filter_matches(&o, "IMAGE", None) && !filter_matches(&o, "MASK", None));
        assert!(filter_matches(&o, "MASK,IMAGE", None) && filter_matches(&o, "MASK", Some("*")));
    }
}
