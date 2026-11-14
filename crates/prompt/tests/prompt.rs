use ludolph_prompt::{skill_query, skill_words};

#[test]
fn skill_queries_follow_the_caret() {
    assert_eq!(skill_query("use $com", 8), Some((4, "com".to_owned())));
    assert_eq!(skill_query("costs 5$", 8), None);
    assert_eq!(skill_query("$a b", 4), None);
}

#[test]
fn skill_mentions_keep_their_positions() {
    let cases: [(&str, &[(&str, bool)]); 4] = [
        (
            "please $commit: then $review",
            &[
                ("please ", false),
                ("$commit", true),
                (": then ", false),
                ("$review", true),
            ],
        ),
        ("pay $5 a$b", &[("pay $5 a$b", false)]),
        (
            "run $x\nnow",
            &[("run ", false), ("$x", true), ("\nnow", false)],
        ),
        (
            "use $unknown-skill",
            &[("use ", false), ("$unknown-skill", true)],
        ),
    ];
    for (text, want) in cases {
        assert_eq!(skill_words(text), want, "{text}");
    }
}
