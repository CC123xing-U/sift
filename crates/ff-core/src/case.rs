//! Unicode 简单小写与 CJK 判定（DESIGN §4.1）。
//!
//! `names_lower` 必须与 `names` **逐字节等长**（否则 `name_off` 偏移失效）：
//! 逐字符 `char::to_lowercase`，**多字符展开**（如 `İ` → `i̇`）或
//! **小写后字节数变化**（如 `K` U+212A → `k`）时保留原字符。

/// 字节串的简单小写副本；长度与输入逐字节相同。
///
/// 非 UTF-8 输入原样返回（防御路径；索引输入均来自 `String`，恒为合法 UTF-8）。
pub fn simple_lowercase(bytes: &[u8]) -> Vec<u8> {
    let s = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return bytes.to_vec(),
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut buf = [0u8; 4];
    for c in s.chars() {
        let ch = lower_char(c).unwrap_or(c);
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
    out
}

/// 单字符简单小写；多字符展开或字节数变化时返回 `None`（调用方保留原字符）。
fn lower_char(c: char) -> Option<char> {
    let mut it = c.to_lowercase();
    let first = it.next()?;
    if it.next().is_some() {
        return None; // 多字符展开（如 İ）
    }
    if first.len_utf8() != c.len_utf8() {
        return None; // 小写后字节数变化（如 K U+212A → k）
    }
    Some(first)
}

/// 名字是否含 CJK 汉字（DESIGN §4.1：设置 `HAS_CJK` 标志并进 `cjk_ids`，
/// 供 M5 拼音 tier 只遍历这部分条目）。
pub fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

/// CJK 汉字区间（不含假名 / 谚文 / 标点：拼音匹配只对汉字有意义）。
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'      // CJK 统一表意文字扩展 A
        | '\u{4E00}'..='\u{9FFF}'    // CJK 统一表意文字基本区
        | '\u{F900}'..='\u{FAFF}'    // CJK 兼容表意文字
        | '\u{20000}'..='\u{2FA1F}'  // 扩展 B–F 及兼容补充
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower(s: &str) -> String {
        String::from_utf8(simple_lowercase(s.as_bytes())).unwrap()
    }

    #[test]
    fn ascii_lowercased() {
        assert_eq!(lower("README.TXT"), "readme.txt");
    }

    #[test]
    fn multi_char_expansion_kept() {
        // İ (U+0130) → "i̇"（i + 组合点），多字符展开 → 保留原字符
        assert_eq!(lower("İ"), "İ");
        assert_eq!(simple_lowercase("İ".as_bytes()).len(), "İ".len());
    }

    #[test]
    fn sharp_s_kept() {
        // ß 已为小写；ẞ (U+1E9E) → ß 字节数由 3 变 2 → 保留原字符
        assert_eq!(lower("ß"), "ß");
        assert_eq!(lower("ẞ"), "ẞ");
    }

    #[test]
    fn byte_len_change_kept() {
        // K (U+212A KELVIN SIGN，3 字节) → 'k'（1 字节）：字节数变化 → 保留
        assert_eq!(lower("\u{212A}"), "\u{212A}");
        assert_eq!(simple_lowercase("\u{212A}".as_bytes()).len(), 3);
        // 普通 ASCII K 正常折叠
        assert_eq!(lower("K"), "k");
    }

    #[test]
    fn normal_multibyte_lowercased() {
        // Ä (2 字节) → ä (2 字节)：长度不变，正常折叠
        assert_eq!(lower("ÄÖÜ"), "äöü");
    }

    #[test]
    fn cjk_unchanged_and_same_len() {
        let s = "中文拼音测试";
        assert_eq!(lower(s), s);
        assert_eq!(simple_lowercase(s.as_bytes()).len(), s.len());
    }

    #[test]
    fn length_invariant_mixed() {
        let s = "AäİßK中ẛΩ.txt";
        let out = simple_lowercase(s.as_bytes());
        assert_eq!(out.len(), s.len(), "小写副本必须逐字节等长");
    }

    #[test]
    fn non_utf8_passthrough() {
        let raw = [0xff, 0xfe, b'a'];
        assert_eq!(simple_lowercase(&raw), raw);
    }

    #[test]
    fn cjk_detection() {
        assert!(has_cjk("中国人"));
        assert!(has_cjk("abc中文"));
        assert!(has_cjk("\u{3400}"));
        assert!(has_cjk("\u{20000}"));
        assert!(!has_cjk("english.txt"));
        assert!(!has_cjk("あいう")); // 平假名不算（拼音只对汉字）
        assert!(!has_cjk("カタカナ"));
        assert!(!has_cjk("한국어"));
        assert!(!has_cjk("，。！")); // CJK 标点不算
                                     // 注意：日文汉字（如 日）本身是 CJK 统一表意文字，会判为 true——符合预期
        assert!(has_cjk("日本語"));
    }
}
