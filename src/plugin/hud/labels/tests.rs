use super::*;

#[test]
fn display_label_labeled_non_next() {
    assert_eq!(
        display_label(CheckpointKind::Split, "jump-pad", false),
        "&ejump-pad &e(split)"
    );
}

#[test]
fn display_label_empty_non_next_shows_kind() {
    assert_eq!(display_label(CheckpointKind::Start, "", false), "&a(start)");
}

#[test]
fn display_label_labeled_next_prefixes_marker() {
    assert_eq!(
        display_label(CheckpointKind::End, "final", true),
        "&e> &cfinal &c(end)"
    );
}

#[test]
fn display_label_empty_next() {
    assert_eq!(
        display_label(CheckpointKind::Pause, "", true),
        "&e> &b(pause)"
    );
}

#[test]
fn display_label_resume_labeled() {
    assert_eq!(
        display_label(CheckpointKind::Resume, "x", false),
        "&6x &6(resume)"
    );
}
