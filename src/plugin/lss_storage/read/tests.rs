use super::*;

#[test]
fn normalize_appends_lss_when_missing() {
    assert_eq!(normalize_lss_filename("foo").as_deref(), Some("foo.lss"));
    assert_eq!(
        normalize_lss_filename("mycategory-v2").as_deref(),
        Some("mycategory-v2.lss")
    );
}

#[test]
fn normalize_keeps_existing_lss_extension() {
    assert_eq!(
        normalize_lss_filename("foo.lss").as_deref(),
        Some("foo.lss")
    );
    // Case-insensitive on the extension check; original case preserved,
    // no double-append.
    assert_eq!(
        normalize_lss_filename("foo.LSS").as_deref(),
        Some("foo.LSS")
    );
}

#[test]
fn normalize_rejects_path_traversal_and_separators() {
    for bad in [
        "",
        ".",
        "..",
        "../escape",
        "../escape.lss",
        "a/b.lss",
        "a\\b.lss",
        "/abs.lss",
        "sub/dir/",
    ] {
        assert_eq!(normalize_lss_filename(bad), None, "should reject {bad:?}");
    }
}
