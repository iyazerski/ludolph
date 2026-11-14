//! Pull-request links mentioned in final assistant answers.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use url::Url;

/// Details derived from a session, without keeping another history store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    /// All recorded compactions, including on earlier branches, as pi counts them.
    pub compactions: usize,
    /// Links on the current branch, deduplicated by canonical PR URL.
    pub pull_requests: Vec<PullRequest>,
}

/// A linked PR, not a claim that it is still open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PullRequest {
    pub url: String,
    pub label: String,
}

/// Extract links without copying thinking, images or tool arguments.
pub(super) fn pull_requests(content: &RawValue) -> Vec<PullRequest> {
    #[derive(Deserialize)]
    struct Block {
        #[serde(rename = "type")]
        kind: String,
        text: Option<String>,
    }
    let Ok(blocks) = serde_json::from_str::<Vec<Block>>(content.get()) else {
        return Vec::new();
    };
    let mut links = Vec::new();
    for block in blocks.into_iter().filter(|block| block.kind == "text") {
        let Some(text) = block.text else { continue };
        let mut code = false;
        for event in Parser::new(&text) {
            match event {
                Event::Start(Tag::CodeBlock(_)) => code = true,
                Event::End(TagEnd::CodeBlock) => code = false,
                Event::Start(Tag::Link { dest_url, .. }) if !code => {
                    links.extend(pull_request(&dest_url));
                }
                Event::Text(text) if !code => {
                    for (at, _) in text.match_indices("https://") {
                        let candidate = text[at..]
                            .split(|c: char| {
                                c.is_whitespace()
                                    || matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '"' | '\'')
                            })
                            .next()
                            .unwrap_or_default();
                        links.extend(pull_request(
                            candidate.trim_end_matches(['.', ',', ';', ':', '!', '?']),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    links
}

fn pull_request(candidate: &str) -> Option<PullRequest> {
    let mut url = Url::parse(candidate).ok()?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    let [owner, repo, "pull", number, ..] = segments.as_slice() else {
        return None;
    };
    let number = number.parse::<u64>().ok().filter(|number| *number > 0)?;
    let label = format!("{owner}/{repo} #{number}");
    let path = format!("/{owner}/{repo}/pull/{number}");
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Some(PullRequest {
        url: url.to_string(),
        label,
    })
}
