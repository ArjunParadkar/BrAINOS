//! Stage 1.1 — the proposal boundary: model text parses into at most one
//! typed proposal, deterministically.

use brainos_mind::proposal::split_reply;

#[test]
fn plain_prose_is_prose() {
    assert_eq!(split_reply("hello there"), ("hello there", None));
    assert_eq!(split_reply("  padded  "), ("padded", None));
    assert_eq!(split_reply(""), ("", None));
}

#[test]
fn say_plus_action_splits_cleanly() {
    let (say, act) = split_reply("on it~~sh|uname -a");
    assert_eq!(say, "on it");
    assert_eq!(act, Some(("sh", "uname -a")));

    let (say, act) = split_reply("looking~~fs.list|");
    assert_eq!(say, "looking");
    assert_eq!(act, Some(("fs.list", "")));
}

#[test]
fn action_without_target_still_types() {
    let (say, act) = split_reply("switching~~ui.presence");
    assert_eq!(say, "switching");
    assert_eq!(act, Some(("ui.presence", "")));
}

#[test]
fn empty_action_suffix_is_no_action() {
    assert_eq!(split_reply("done~~"), ("done", None));
    assert_eq!(split_reply("done~~   "), ("done", None));
}

#[test]
fn whitespace_around_verb_and_target_is_trimmed() {
    let (_, act) = split_reply("x~~  sh  |  echo hi  ");
    assert_eq!(act, Some(("sh", "echo hi")));
}
