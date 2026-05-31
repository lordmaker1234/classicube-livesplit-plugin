use super::*;

#[test]
fn strips_color_codes() {
    assert_eq!(sanitize_component("&aMyMap"), "MyMap");
    assert_eq!(sanitize_component("Map&7Name"), "MapName");
    assert_eq!(sanitize_component("&a&b&c"), "_");
}

#[test]
fn strips_trailing_lone_ampersand() {
    assert_eq!(sanitize_component("hello&"), "hello");
}

#[test]
fn maps_unsafe_chars_to_underscore() {
    assert_eq!(sanitize_component("a/b\\c"), "a_b_c");
    assert_eq!(sanitize_component("foo bar"), "foo_bar");
    assert_eq!(sanitize_component("a:b*c?d"), "a_b_c_d");
}

#[test]
fn collapses_runs_of_underscores() {
    assert_eq!(sanitize_component("a   b"), "a_b");
    assert_eq!(sanitize_component("a___b"), "a_b");
}

#[test]
fn trims_leading_and_trailing_underscores() {
    assert_eq!(sanitize_component("___hello___"), "hello");
    assert_eq!(sanitize_component("   foo   "), "foo");
}

#[test]
fn preserves_dash_dot_underscore() {
    assert_eq!(sanitize_component("foo.bar-baz_qux"), "foo.bar-baz_qux");
}

#[test]
fn caps_at_64_chars() {
    let long = "a".repeat(100);
    let out = sanitize_component(&long);
    assert_eq!(out.len(), 64);
    assert!(out.chars().all(|c| c == 'a'));
}

#[test]
fn cap_falls_on_char_boundary_for_unicode_input() {
    // Multi-byte chars get mapped to `_` (outside ASCII alphanumeric),
    // collapsed, and trimmed; the cap operates on the post-mapping
    // ASCII string and never lands mid-codepoint.
    let s = "\u{1F600}".repeat(80);
    let out = sanitize_component(&s);
    // After mapping each emoji to '_', collapsing leaves a single '_',
    // which then gets trimmed -> empty -> placeholder "_".
    assert_eq!(out, "_");
}

#[test]
fn empty_input_yields_placeholder() {
    assert_eq!(sanitize_component(""), "_");
    assert_eq!(sanitize_component("   "), "_");
    assert_eq!(sanitize_component("///"), "_");
}

#[test]
fn singleplayer_caller_is_responsible_for_placeholder() {
    // The sanitizer itself doesn't know about the singleplayer
    // convention; the caller substitutes the literal before calling.
    assert_eq!(sanitize_component("singleplayer"), "singleplayer");
}

#[test]
fn track_dir_composes_components() {
    let dir = track_dir("My Server", "Lobby/1");
    let s = dir.to_string_lossy();
    assert!(s.contains("plugins"));
    assert!(s.contains("livesplit"));
    assert!(s.contains("My_Server"));
    assert!(s.contains("Lobby_1"));
}

#[test]
fn list_versions_returns_empty_for_missing_dir() {
    let dir = std::env::temp_dir().join(format!(
        "lss-storage-missing-{}-{}",
        std::process::id(),
        line!()
    ));
    assert!(
        !dir.exists(),
        "test precondition: {} should not exist",
        dir.display()
    );
    assert!(list_versions(&dir, "any").is_empty());
}

#[test]
fn list_versions_parses_and_sorts() {
    let tmp = std::env::temp_dir().join(format!("lss-list-versions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    for name in [
        "cat-v1.lss",
        "cat-v10.lss",
        "cat-v2.lss",
        "cat-v.lss",
        "cat.lss",
        "other-v1.lss",
        "cat-v1.txt",
        "cat-v3.lss",
    ] {
        std::fs::write(tmp.join(name), b"x").unwrap();
    }

    let versions = list_versions(&tmp, "cat");
    let nums: Vec<u32> = versions.iter().map(|(v, _)| *v).collect();
    assert_eq!(nums, vec![1, 2, 3, 10]);

    std::fs::remove_dir_all(&tmp).unwrap();
}
