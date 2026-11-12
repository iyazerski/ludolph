//! The archive rule: each project lists only its most recent chats
//! (`docs/design.md`, "Archiving").
//!
//! A chat's mark holds until the chat has newer activity. So an archived
//! chat comes back when it's used again, and a chat you unarchived stays
//! listed until then, even if the rule would archive it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::index::{Index, Mark};
use crate::session::Summary;

/// When a chat was last active, for ordering and for its mark.
pub fn activity(summary: &Summary) -> DateTime<Utc> {
    summary.last_activity.unwrap_or(summary.created)
}

/// Chat `summary`'s mark, if it still holds.
pub fn mark(index: &Index, summary: &Summary) -> Option<Mark> {
    index
        .mark(&summary.id)
        .filter(|mark| mark.at >= activity(summary))
}

/// Archive what the rule says to, and drop marks that no longer hold.
/// Chats whose folder isn't a project (`is_project`) form one group, and
/// chats in `busy` stay. Returns whether any mark changed.
pub fn apply(
    index: &mut Index,
    keep: usize,
    busy: &HashSet<&str>,
    is_project: impl Fn(&Path) -> bool,
) -> bool {
    let mut changes: Vec<(String, Option<Mark>)> = Vec::new();
    {
        let index = &*index;
        let summaries: HashMap<&str, &Summary> =
            index.chats().map(|(_, s)| (s.id.as_str(), s)).collect();
        let stale = index
            .marked()
            .filter(|id| summaries.get(id).is_none_or(|s| mark(index, s).is_none()));
        changes.extend(stale.map(|id| (id.to_owned(), None)));

        let mut groups: HashMap<Option<&Path>, Vec<&Summary>> = HashMap::new();
        for summary in summaries.into_values() {
            let listed = mark(index, summary).is_none_or(|mark| !mark.archived);
            if listed {
                let group = is_project(&summary.cwd).then_some(summary.cwd.as_path());
                groups.entry(group).or_default().push(summary);
            }
        }
        for mut group in groups.into_values() {
            group.sort_by(|a, b| activity(b).cmp(&activity(a)).then_with(|| a.id.cmp(&b.id)));
            let older = group.into_iter().skip(keep);
            // A chat you unarchived has a mark; it and busy chats stay.
            let chosen =
                older.filter(|s| mark(index, s).is_none() && !busy.contains(s.id.as_str()));
            changes.extend(chosen.map(|s| {
                let mark = Mark {
                    archived: true,
                    at: activity(s),
                };
                (s.id.clone(), Some(mark))
            }));
        }
    }
    let mut changed = false;
    for (id, mark) in changes {
        changed |= index.set_mark(&id, mark);
    }
    changed
}
