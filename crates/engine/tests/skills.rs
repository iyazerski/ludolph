use ludolph_engine::skills::{as_typed, to_front};

#[test]
fn a_skill_command_moves_to_the_front() {
    let cases = [
        ("/skill:a fix it", "/skill:a fix it"),
        ("fix it with /skill:a", "/skill:a fix it with"),
        ("please /skill:a now", "/skill:a please now"),
        (
            "look:\n\n/skill:a\nthen this",
            "/skill:a look:\n\nthen this",
        ),
        (
            "please\n/skill:a\n\nthen this",
            "/skill:a please\n\nthen this",
        ),
        ("/skill:a and /skill:b", "/skill:a and /skill:b"),
        ("one /skill:a two /skill:b", "/skill:a one two /skill:b"),
        ("see a/skill:b", "see a/skill:b"),
        ("just /skill: alone", "just /skill: alone"),
        ("no skill here", "no skill here"),
    ];
    for (text, sent) in cases {
        assert_eq!(to_front(text), sent, "{text:?}");
    }
}

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
