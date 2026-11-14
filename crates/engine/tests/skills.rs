use ludolph_engine::skills::as_typed;

#[test]
fn expanded_skills_keep_their_original_positions() {
    let block = "<skill name=\"commit\" location=\"/s/commit/SKILL.md\">\nReferences are relative to /s/commit.\n\nCommit it.\n</skill>";
    let cases = [
        (
            format!("{block}\n\nand push"),
            "$commit\n\nand push".to_owned(),
        ),
        (block.to_owned(), "$commit".to_owned()),
        (
            format!("please {block}, then {block}."),
            "please $commit, then $commit.".to_owned(),
        ),
        (
            "<skill name=\"x\" location=\"/x\">\nsay </skill>\nhi\n</skill>\n\ngo".to_owned(),
            "$x\n\ngo".to_owned(),
        ),
        (
            "<skill name=\"x\">\nno location\n</skill>".to_owned(),
            "<skill name=\"x\">\nno location\n</skill>".to_owned(),
        ),
        ("a plain prompt".to_owned(), "a plain prompt".to_owned()),
    ];
    for (text, typed) in &cases {
        assert_eq!(as_typed(text), *typed, "{text:?}");
    }
}
