use ludolph_prompt::to_front;

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
