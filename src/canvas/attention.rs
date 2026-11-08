//! Ctrl+Up / Ctrl+Down prompt weighting — a port of the web frontend's
//! `extensions/core/editAttention.ts` (`(word:1.05)`, nested parens, weight 1 unwraps).
//! Works on char indices (GTK text iters count chars, not bytes).

const DELIMS: &str = " .,\\/!?%^*;:{}=-_`~()\r\n\t";

/// `incrementWeight`: `"1.0" + 0.05 = "1.05"`, `"1.05" - 0.05 = "1"`.
fn increment(weight: &str, delta: f64) -> String {
    let Ok(w) = weight.parse::<f64>() else { return weight.to_owned() };
    let s = format!("{:.10}", w + delta);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

/// A trailing `:number` weight (not a time like `12:30`) → (inner text, weight).
fn split_weight(inner: &str) -> Option<(&str, &str)> {
    let i = inner.rfind(':')?;
    let (head, w) = (&inner[..i], &inner[i + 1..]);
    let numeric = !w.is_empty()
        && w.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c))
        && w.chars().any(|c| c.is_ascii_digit())
        && w.parse::<f64>().is_ok();
    if !numeric {
        return None;
    }
    // `(?:^|\s)\d{1,2}:\d{2}$` is a time, not a weight.
    let digits = head.chars().rev().take_while(char::is_ascii_digit).count();
    let before = head.chars().rev().nth(digits);
    let time = w.len() == 2 && w.chars().all(|c| c.is_ascii_digit()) && (1..=2).contains(&digits) && before.is_none_or(char::is_whitespace);
    (!time).then_some((head, w))
}

/// `findNearestEnclosure`: the paren group around `cursor` (exclusive of the parens).
fn enclosure(t: &[char], cursor: usize) -> Option<(usize, usize)> {
    let (mut start, mut end) = (cursor as isize, cursor);
    let (mut open, mut close) = (0, 0);
    if t.get(cursor) == Some(&'(') {
        end = cursor + 1;
    } else {
        loop {
            start -= 1;
            if start < 0 {
                return None;
            }
            match t[start as usize] {
                '(' if open == close => break,
                '(' => open += 1,
                ')' => close += 1,
                _ => {}
            }
        }
        (open, close) = (0, 0);
    }
    while end < t.len() {
        match t[end] {
            ')' if open == close => break,
            '(' => open += 1,
            ')' => close += 1,
            _ => {}
        }
        end += 1;
    }
    (end < t.len()).then_some(((start + 1) as usize, end))
}

/// Apply one step to `text` with selection `[start, end)` (char offsets).
/// Returns the new text and the char range of the edited group (to reselect it).
pub fn edit(text: &str, (mut start, mut end): (usize, usize), delta: f64) -> Option<(String, (usize, usize))> {
    let t: Vec<char> = text.chars().collect();
    let (s0, e0) = (start.min(end).min(t.len()), end.max(start).min(t.len()));
    (start, end) = (s0, e0);
    if start == end {
        if let Some((s, e)) = enclosure(&t, start) {
            (start, end) = (s, e);
        } else {
            while start > 0 && !DELIMS.contains(t[start - 1]) {
                start -= 1;
            }
            while end < t.len() && !DELIMS.contains(t[end]) {
                end += 1;
            }
            if start == end {
                return None;
            }
        }
    }
    if end > start && t[end - 1] == ' ' {
        end -= 1;
    }
    if start > 0 && end < t.len() && t[start - 1] == '(' && t[end] == ')' {
        start -= 1;
        end += 1;
    }
    let mut sel: String = t[start..end].iter().collect();
    if !(sel.starts_with('(') && sel.ends_with(')') && sel.chars().count() >= 2) {
        sel = format!("({sel})");
    }
    let inner = &sel[1..sel.len() - 1];
    let (body, weight) = split_weight(inner).unwrap_or((inner, "1.0"));
    let w = increment(weight, delta);
    let new = if w == "1" { body.to_owned() } else { format!("({body}:{w})") };
    let out: String = t[..start].iter().copied().chain(new.chars()).chain(t[end..].iter().copied()).collect();
    Some((out, (start, start + new.chars().count())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_like_the_web_ui() {
        assert_eq!(increment("1.0", 0.05), "1.05");
        assert_eq!(increment("1.05", -0.05), "1");
        assert_eq!(increment("0.05", -0.05), "0");
        // cursor inside a word
        assert_eq!(edit("a cat, dog", (3, 3), 0.05).unwrap().0, "a (cat:1.05), dog");
        // inside an existing group
        assert_eq!(edit("a (cat:1.05), dog", (5, 5), 0.05).unwrap().0, "a (cat:1.1), dog");
        // back to 1 unwraps
        assert_eq!(edit("(cat:1.05)", (2, 2), -0.05).unwrap().0, "cat");
        // selection
        let (t, sel) = edit("red hair, blue", (0, 8), 0.1).unwrap();
        assert_eq!(t, "(red hair:1.1), blue");
        assert_eq!(sel, (0, 14));
        // unicode + times are not weights
        assert_eq!(edit("été", (1, 1), 0.05).unwrap().0, "(été:1.05)");
        assert_eq!(split_weight("at 12:30"), None);
        assert_eq!(split_weight("v2:1.5"), Some(("v2", "1.5")));
        assert_eq!(edit(" , ", (1, 1), 0.05), None);
    }
}
