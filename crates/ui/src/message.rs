//! The composer's `$` skill words: finding the one at the caret, sending
//! each as the command that runs it, and marking them in a sent message.

use super::data::Skill;

/// Whether `c` can be in a skill's name after the `$`.
fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '-')
}

/// The `$` word the caret at byte `caret` ends: where its `$` is, and the
/// lowercased text after it.
pub fn skill_query(text: &str, caret: usize) -> Option<(usize, String)> {
    let before = text.get(..caret)?;
    let start = before.rfind('$')?;
    let word = &before[start + 1..];
    let lead = before[..start].chars().next_back();
    let ok = lead.is_none_or(char::is_whitespace) && word.chars().all(name_char);
    ok.then(|| (start, word.to_lowercase()))
}

/// `text` with each `$name` of a known skill as the command that runs it.
pub fn with_skill_refs(text: &str, skills: &[Skill]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('$') {
        let lead_ok = rest[..i].chars().next_back().map_or_else(
            || out.chars().next_back().is_none_or(char::is_whitespace),
            char::is_whitespace,
        );
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let len = after.find(|c| !name_char(c)).unwrap_or(after.len());
        let name = after[..len].trim_end_matches(':');
        match skills
            .iter()
            .find(|s| s.name == name)
            .filter(|_| lead_ok && !name.is_empty())
        {
            Some(skill) => {
                out.push_str(&skill.reference);
                rest = &after[name.len()..];
            }
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` cut at its `$name` words, as the engine shows a skill that ran:
/// `(piece, is_skill)`. A name starts with a letter, so prices stay text.
pub fn skill_words(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let (mut plain, mut at) = (0, 0);
    while let Some(i) = text[at..].find('$').map(|i| i + at) {
        let lead_ok = text[..i]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let after = &text[i + 1..];
        let len = after.find(|c| !name_char(c)).unwrap_or(after.len());
        let name = after[..len].trim_end_matches(':');
        if lead_ok && name.starts_with(char::is_alphabetic) {
            if i > plain {
                out.push((&text[plain..i], false));
            }
            let end = i + 1 + name.len();
            out.push((&text[i..end], true));
            (plain, at) = (end, end);
        } else {
            at = i + 1;
        }
    }
    if plain < text.len() {
        out.push((&text[plain..], false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_words_complete_and_expand() {
        assert_eq!(skill_query("use $com", 8), Some((4, "com".to_owned())));
        assert_eq!(skill_query("costs 5$", 8), None);
        assert_eq!(skill_query("$a b", 4), None);
        let skills = [Skill {
            name: "commit".into(),
            description: String::new(),
            reference: "/skill:commit".into(),
        }];
        assert_eq!(
            with_skill_refs("$commit then $other, a$commit", &skills),
            "/skill:commit then $other, a$commit"
        );
    }

    #[test]
    fn sent_skills_stand_out() {
        let cases: [(&str, &[(&str, bool)]); 3] = [
            (
                "$commit: then push",
                &[("$commit", true), (": then push", false)],
            ),
            ("pay $5 a$b", &[("pay $5 a$b", false)]),
            (
                "run $x\nnow",
                &[("run ", false), ("$x", true), ("\nnow", false)],
            ),
        ];
        for (text, want) in cases {
            assert_eq!(skill_words(text), want, "{text}");
        }
    }
}
