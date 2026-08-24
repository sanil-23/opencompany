//! Search-result provenance for workspace attribution.
//!
//! The managed `web_search` tool ([`search`](super::search)) hands an agent
//! third-party sources it is expected to cite. When that agent then puts a
//! document into the company workspace, the operator reading it has no way to
//! tell whether it was grounded in those searched sources or written from the
//! model's memory. This module closes that gap with an attribution footer —
//! but only on **evidence**, never on a timer:
//!
//! * [`SearchProvenance`] is one per-agent record of the result URLs the
//!   managed search tool actually returned to that agent. The search tool
//!   [`record`](SearchProvenance::record)s exactly the results it rendered.
//! * A workspace note gets the footer **iff its body cites at least one of
//!   those URLs**. The agent roster is cached across turns
//!   ([`HarnessPool`](super::HarnessPool)), so any "searched recently" flag
//!   would stamp documents for days; a URL match is the only signal that ties
//!   *this* document to *those* results.
//!
//! The footer names Exa because the managed backend's `web_search` surface is
//! served by Exa (`tinyhumansai/backend` `WEB_SEARCH_PROVIDER`, default
//! `exa`); the wire response does not yet attribute a provider per call, so
//! this is a product-level statement, not a per-response one. If the backend
//! ever grows a response-level `provider` field, the footer text is the one
//! place to consult it.
//!
//! Deliberately precise over complete: a document that paraphrases searched
//! sources without citing a single returned URL gets no footer. The failure
//! mode this module refuses is the opposite one — a footer on a document that
//! never touched a search result.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// The attribution line appended to a workspace note grounded in managed
/// search results.
pub const ATTRIBUTION_FOOTER: &str = "*Powered by Exa*";

/// How many result URLs are retained, oldest evicted first.
///
/// Agents are long-lived (the roster is cached across turns), so the record is
/// bounded. 256 URLs is ~25 maximally-sized searches — far more history than
/// any draft-then-write flow spans — while keeping the per-write scan trivial.
const MAX_TRACKED_URLS: usize = 256;

/// One agent's record of the search-result URLs the managed `web_search` tool
/// returned to it. Shared (`Arc`) between that agent's search tool and its
/// workspace write tools.
pub struct SearchProvenance {
    urls: Mutex<VecDeque<String>>,
}

impl SearchProvenance {
    /// An empty record, ready to share between one agent's tools.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            urls: Mutex::new(VecDeque::new()),
        })
    }

    /// Record result URLs the agent was shown, newest kept on eviction.
    ///
    /// URLs are stored normalized ([`normalize_url`]); duplicates refresh
    /// nothing and are skipped, so a repeated search cannot flush older
    /// evidence out of the window.
    pub fn record<'a>(&self, urls: impl IntoIterator<Item = &'a str>) {
        let mut tracked = self.urls.lock().expect("search provenance lock");
        for url in urls {
            let Some(normalized) = normalize_url(url) else {
                continue;
            };
            if tracked.contains(&normalized) {
                continue;
            }
            if tracked.len() == MAX_TRACKED_URLS {
                tracked.pop_front();
            }
            tracked.push_back(normalized);
        }
    }

    /// Whether `content` cites at least one recorded result URL.
    pub fn cited_in(&self, content: &str) -> bool {
        let tracked = self.urls.lock().expect("search provenance lock");
        tracked.iter().any(|url| cites(content, url))
    }

    /// `content` with the attribution footer appended, when it has earned one:
    /// it cites a recorded URL and does not already carry the footer.
    /// `None` means "store the body as given" — either the evidence is absent
    /// or the footer is already there.
    pub fn attributed(&self, content: &str) -> Option<String> {
        if content.contains(ATTRIBUTION_FOOTER) || !self.cited_in(content) {
            return None;
        }
        Some(format!(
            "{body}\n\n---\n{ATTRIBUTION_FOOTER}\n",
            body = content.trim_end()
        ))
    }
}

/// A URL in the shape worth remembering: scheme-qualified, with the trailing
/// slash dropped so `…/docs` and `…/docs/` are one entry. Anything else —
/// empty, relative, or junk the backend should not have sent — is `None`.
fn normalize_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() || !url.contains("://") {
        return None;
    }
    Some(url.to_string())
}

/// Whether `content` contains `url` as a citation rather than as a prefix of
/// some other URL.
///
/// A plain substring check would let a recorded `https://exa.ai` claim credit
/// for a cited `https://exa.ai.evil.example`. An occurrence counts only when
/// the character after it cannot be *extending the same URL component*: a
/// letter or digit extends it outright, and a `.` followed by a letter or
/// digit is a longer hostname (while `.` followed by anything else is sentence
/// punctuation). A `/` after the match is a deeper path under the recorded
/// result and counts — citing a page found *via* the result is still citing
/// the result.
fn cites(content: &str, url: &str) -> bool {
    let mut from = 0;
    while let Some(at) = content[from..].find(url) {
        let end = from + at + url.len();
        let mut rest = content[end..].chars();
        match rest.next() {
            None => return true,
            Some(next) if next.is_ascii_alphanumeric() => {}
            Some('.') => match rest.next() {
                Some(after_dot) if after_dot.is_ascii_alphanumeric() => {}
                _ => return true,
            },
            Some(_) => return true,
        }
        from = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance_with(urls: &[&str]) -> Arc<SearchProvenance> {
        let p = SearchProvenance::new();
        p.record(urls.iter().copied());
        p
    }

    #[test]
    fn a_doc_citing_a_recorded_url_earns_the_footer() {
        let p = provenance_with(&["https://exa.ai/docs/reference/search"]);
        let doc = "See https://exa.ai/docs/reference/search for the API.";
        let out = p.attributed(doc).expect("footer expected");
        assert!(out.starts_with(doc), "{out}");
        assert!(out.trim_end().ends_with(ATTRIBUTION_FOOTER), "{out}");
    }

    #[test]
    fn a_doc_citing_nothing_recorded_gets_no_footer() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        assert!(p.attributed("All my own thoughts.").is_none());
        assert!(
            p.attributed("Cites https://example.com/other instead.")
                .is_none()
        );
    }

    #[test]
    fn no_recorded_searches_means_no_footer_ever() {
        let p = SearchProvenance::new();
        assert!(
            p.attributed("Even https://exa.ai/docs cited raw.")
                .is_none()
        );
    }

    #[test]
    fn a_body_already_carrying_the_footer_is_left_alone() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        let doc = format!("From https://exa.ai/docs.\n\n---\n{ATTRIBUTION_FOOTER}\n");
        assert!(p.attributed(&doc).is_none());
    }

    #[test]
    fn trailing_slash_and_deeper_paths_still_count() {
        let p = provenance_with(&["https://exa.ai/docs/"]);
        assert!(p.cited_in("read https://exa.ai/docs today"));
        assert!(p.cited_in("read https://exa.ai/docs/reference today"));
    }

    #[test]
    fn a_longer_host_gets_no_credit() {
        let p = provenance_with(&["https://exa.ai"]);
        assert!(!p.cited_in("see https://exa.ai.evil.example/page"));
        assert!(!p.cited_in("see https://exa.aique.example/page"));
        // …while genuine end-of-sentence punctuation does.
        assert!(p.cited_in("see https://exa.ai. Next sentence."));
        assert!(p.cited_in("see (https://exa.ai)"));
    }

    #[test]
    fn the_window_is_bounded_and_dedupes() {
        let p = SearchProvenance::new();
        let urls: Vec<String> = (0..MAX_TRACKED_URLS + 10)
            .map(|n| format!("https://example.com/{n}"))
            .collect();
        p.record(urls.iter().map(String::as_str));
        p.record(["https://example.com/5"]); // dupe: must not evict anything
        assert!(!p.cited_in("https://example.com/9")); // evicted
        assert!(p.cited_in(&format!("https://example.com/{}", MAX_TRACKED_URLS + 9)));
    }

    #[test]
    fn junk_urls_are_never_recorded() {
        let p = provenance_with(&["", "   ", "not-a-url", "/relative/path"]);
        assert!(!p.cited_in("not-a-url and /relative/path in text"));
    }
}
