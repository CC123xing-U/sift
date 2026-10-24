//! M1 核心跨 crate 集成验收（DESIGN §13-M1）。
//! 纯内存、无管理员权限要求；真实 NTFS 卷测试见 `windows_integration.rs`。

use ff_core::{
    attrs, parse, simple_lowercase, CancelToken, EntryInput, IndexBuilder, VolumeIndex, VolumeInfo,
};

fn entry(frn: u64, parent_frn: u64, name: &str, is_dir: bool) -> EntryInput {
    EntryInput {
        frn,
        parent_frn,
        name: name.to_string(),
        is_dir,
        attrs: if is_dir {
            attrs::FILE_ATTRIBUTE_DIRECTORY
        } else {
            0
        },
        size: 0,
        mtime: 0,
        ctime: 0,
        atime: 0,
    }
}

fn index(entries: impl IntoIterator<Item = EntryInput>) -> VolumeIndex {
    let mut builder = IndexBuilder::new(VolumeInfo {
        drive: Some('C'),
        ..Default::default()
    });
    for item in entries {
        builder.push(item);
    }
    builder.finish()
}

#[test]
fn lowercase_preserves_byte_layout_for_unicode_names() {
    let names = [
        "README.TXT",
        "İstanbul.txt",
        "Straße-ẞ.txt",
        "中文报告.TXT",
        "ÄÖÜ-Ω.txt",
        "K-kelvin-\u{212A}.txt",
    ];
    let ix = index(
        std::iter::once(entry(1, 0, ".", true)).chain(
            names
                .iter()
                .enumerate()
                .map(|(i, name)| entry(10 + i as u64, 1, name, false)),
        ),
    );

    assert_eq!(ix.names.len(), ix.names_lower.len());
    for (id, original) in (1..).zip(names) {
        let lower = simple_lowercase(original.as_bytes());
        assert_eq!(lower.len(), original.len());
        assert_eq!(ix.name(id), original.as_bytes());
        assert_eq!(
            &ix.names_lower[ix.name_off[id as usize] as usize..][..original.len()],
            lower
        );
    }
}

#[test]
fn deep_paths_and_cycles_fail_cleanly() {
    let mut entries = vec![entry(1, 0, ".", true)];
    for frn in 2..=500 {
        entries.push(entry(frn, frn - 1, &format!("d{frn}"), true));
    }
    entries.push(entry(501, 500, "深层文档.txt", false));
    let deep = index(entries);
    let path = deep.path_string(500).expect("512 层以内的路径应可生成");
    assert!(path.starts_with("C:\\d2\\d3\\"));
    assert!(path.ends_with("\\深层文档.txt"));

    let cyclic = index([
        entry(10, 20, "a", true),
        entry(20, 10, "b", true),
        entry(30, 30, "self", true),
    ]);
    let mut buf = b"stale".to_vec();
    for id in 0..3 {
        assert!(!cyclic.write_path(id, &mut buf));
        assert!(buf.is_empty(), "失败时不得留下半条路径");
    }
}

#[test]
fn and_query_uses_rare_driver_and_verifies_all_terms() {
    let ix = index([
        entry(1, 0, ".", true),
        entry(2, 1, "alpha common", false),
        entry(3, 1, "beta common", false),
        entry(4, 1, "zz-rare-token common", false),
        entry(5, 1, "zz-rare-token other", false),
    ]);
    let token = CancelToken::new();
    let query = parse("common zz-rare-token").unwrap();
    assert_eq!(ix.search(&query, &token), vec![3]);
    assert!(ix
        .search(&parse("common missing").unwrap(), &token)
        .is_empty());
}

#[test]
fn empty_and_unicode_queries_have_stable_results() {
    let ix = index([
        entry(1, 0, ".", true),
        entry(2, 1, "用户", true),
        entry(3, 2, "报告 Final.TXT", false),
        entry(4, 2, " café-au-lait.txt", false),
    ]);
    let token = CancelToken::new();
    for empty in ["", "   \t ", "\"\""] {
        assert!(ix.search(&parse(empty).unwrap(), &token).is_empty());
    }
    assert_eq!(ix.search(&parse("报告 final").unwrap(), &token), vec![2]);
    assert_eq!(ix.search(&parse("用户").unwrap(), &token), vec![1]);
    assert_eq!(ix.search(&parse("CAFÉ").unwrap(), &token), vec![3]);
    assert!(ix
        .search(&parse("不存在的名字").unwrap(), &token)
        .is_empty());
}
