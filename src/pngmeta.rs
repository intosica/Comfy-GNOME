// SPDX-License-Identifier: GPL-3.0-only
//! Read the uncompressed `tEXt` / `iTXt` chunks of a PNG (ComfyUI embeds the graph in the
//! `workflow` chunk and the API prompt in `prompt`).

const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

pub fn is_png(b: &[u8]) -> bool {
    b.starts_with(SIG)
}

/// All (keyword, text) pairs; compressed chunks are skipped.
pub fn text_chunks(b: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !is_png(b) {
        return out;
    }
    let mut i = SIG.len();
    while i + 8 <= b.len() {
        let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let ty = &b[i + 4..i + 8];
        let Some(data) = b.get(i + 8..i + 8 + len) else { break };
        match ty {
            b"tEXt" => {
                if let Some(z) = data.iter().position(|&c| c == 0) {
                    // Latin-1 per spec.
                    let text: String = data[z + 1..].iter().map(|&c| c as char).collect();
                    out.push((String::from_utf8_lossy(&data[..z]).into_owned(), text));
                }
            }
            b"iTXt" => {
                if let Some(z) = data.iter().position(|&c| c == 0) {
                    let rest = &data[z + 1..];
                    if rest.len() >= 2 && rest[0] == 0 {
                        // skip compression method, language tag\0, translated keyword\0
                        let mut it = rest[2..].splitn(3, |&c| c == 0);
                        if let (Some(_), Some(_), Some(text)) = (it.next(), it.next(), it.next()) {
                            out.push((String::from_utf8_lossy(&data[..z]).into_owned(), String::from_utf8_lossy(text).into_owned()));
                        }
                    }
                }
            }
            b"IEND" => break,
            _ => {}
        }
        i += 12 + len;
    }
    out
}

/// The embedded workflow JSON, if any.
pub fn workflow(b: &[u8]) -> Option<String> {
    text_chunks(b).into_iter().find(|(k, _)| k == "workflow").map(|(_, v)| v)
}

/// `(workflow, prompt)` JSON that ComfyUI stores as container tags in MP4/WebM/MKV/GIF-like
/// files. Scans the head and tail of the file for the key followed by a JSON object.
pub fn video_meta(path: &std::path::Path) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 16 << 20;
    let Ok(mut f) = std::fs::File::open(path) else { return (None, None) };
    let len = f.metadata().map_or(0, |m| m.len());
    let mut parts = Vec::new();
    for (start, n) in if len <= 2 * CHUNK { vec![(0, len)] } else { vec![(0, CHUNK), (len - CHUNK, CHUNK)] } {
        let mut b = vec![0; n as usize];
        if f.seek(SeekFrom::Start(start)).and_then(|_| f.read_exact(&mut b)).is_ok() {
            parts.push(b);
        }
    }
    let find = |key: &[u8]| {
        parts.iter().find_map(|b| {
            b.windows(key.len()).enumerate().filter(|(_, w)| *w == key).find_map(|(i, _)| {
                let rest = &b[i + key.len()..];
                let at = rest.iter().take(48).position(|&c| c == b'{')?;
                serde_json::Deserializer::from_slice(&rest[at..]).into_iter::<serde_json::Value>().next()?.ok().filter(|v| v.is_object())
            })
        })
    };
    (find(b"workflow"), find(b"prompt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(ty: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(ty);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0; 4]); // CRC not checked
        v
    }

    #[test]
    fn finds_workflow_in_text_and_itxt() {
        let mut png = SIG.to_vec();
        png.extend(chunk(b"tEXt", b"prompt\0{\"a\":1}"));
        png.extend(chunk(b"tEXt", b"workflow\0{\"nodes\":[]}"));
        png.extend(chunk(b"IEND", b""));
        assert_eq!(workflow(&png).as_deref(), Some("{\"nodes\":[]}"));
        let mut p2 = SIG.to_vec();
        p2.extend(chunk(b"iTXt", b"workflow\0\0\0\0\0{\"x\":\"\xc3\xa9\"}"));
        assert_eq!(workflow(&p2).as_deref(), Some("{\"x\":\"é\"}"));
        assert_eq!(workflow(b"nope"), None);
    }
}
