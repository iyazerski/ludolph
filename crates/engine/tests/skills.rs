use ludolph_engine::skills::as_typed;

#[test]
fn an_expanded_skill_shows_as_typed() {
    let block = "<skill name=\"commit\" location=\"/s/commit/SKILL.md\">\nReferences are relative to /s/commit.\n\nCommit it.\n</skill>";
    let cases = [
        (format!("{block}\n\nand push"), "/skill:commit and push"),
        (block.to_owned(), "/skill:commit"),
        // A `</skill>` inside the skill's own text doesn't end it.
        (
            "<skill name=\"x\" location=\"/x\">\nsay </skill>\nhi\n</skill>\n\ngo".to_owned(),
            "/skill:x go",
        ),
        (
            "<skill name=\"x\">\nno location\n</skill>".to_owned(),
            "<skill name=\"x\">\nno location\n</skill>",
        ),
        (format!("{block} and push"), &*format!("{block} and push")),
        ("a plain prompt".to_owned(), "a plain prompt"),
    ];
    for (text, typed) in &cases {
        assert_eq!(as_typed(text), *typed, "{text:?}");
    }
}
