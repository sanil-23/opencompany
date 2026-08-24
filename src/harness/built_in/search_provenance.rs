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

/// The complete terminal block [`SearchProvenance::attributed`] appends: a
/// horizontal rule and the attribution line.
///
/// Detection matches on **this**, anchored to the end of the body, rather than
/// on [`ATTRIBUTION_FOOTER`] anywhere in it. A document that merely discusses
/// the footer — a style guide quoting `*Powered by Exa*`, or a brief about this
/// very feature — would otherwise be read as already attributed and silently
/// stored without the credit it earned.
const ATTRIBUTION_BLOCK: &str = "---\n*Powered by Exa*";

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
        let carries = carries_attribution(content);
        match (self.cited_in(content), carries) {
            // Earned and already stamped: store exactly as given.
            (true, true) => None,
            // Earned: stamp it.
            (true, false) => Some(format!(
                "{body}\n\n{ATTRIBUTION_BLOCK}\n",
                body = content.trim_end()
            )),
            // NOT earned but stamped anyway — a body the model wrote the footer
            // into itself, or a revision that dropped its last citation while
            // keeping the old footer. The claim is the host's to make, not the
            // model's, so it is removed rather than trusted: an attribution
            // that survives without evidence is exactly the false stamp this
            // module exists to prevent.
            (false, true) => Some(without_attribution(content)),
            (false, false) => None,
        }
    }
}

/// Whether `content` already ends with the attribution block, and so must be
/// stored as given rather than footered twice.
///
/// Anchored to the end after trimming trailing whitespace: that is where
/// [`SearchProvenance::attributed`] puts it, and matching anywhere would let a
/// passing mention of the phrase suppress a footer the document has earned.
pub fn carries_attribution(content: &str) -> bool {
    content.trim_end().ends_with(ATTRIBUTION_BLOCK)
}

/// A URL in the shape worth remembering: scheme-qualified, with the trailing
/// slash dropped so `…/docs` and `…/docs/` are one entry. Anything else —
/// empty, relative, or junk the backend should not have sent — is `None`.
fn normalize_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || !url.contains("://") {
        return None;
    }
    // Only ONE slash, and only a *path* slash. `trim_end_matches` would eat
    // every trailing slash, and applied to the whole URL it would also eat a
    // slash that is part of a query or fragment value — turning
    // `…/path/?next=/` into a URL the backend never returned.
    let normalized = match url.split_once(['?', '#']) {
        Some(_) => url,
        None => url.strip_suffix('/').unwrap_or(url),
    };
    Some(normalized.to_string())
}

/// `content` with a trailing attribution block removed, and the body's own
/// trailing whitespace left as it was found.
fn without_attribution(content: &str) -> String {
    let trimmed = content.trim_end();
    match trimmed.strip_suffix(ATTRIBUTION_BLOCK) {
        Some(body) => body.trim_end().to_string(),
        None => content.to_string(),
    }
}

/// Whether `next` could be *continuing* a URL that the recorded one is only a
/// prefix of.
///
/// Everything RFC 3986 allows inside a host, port, path, query or fragment and
/// that prose does not normally place immediately after a URL. Deliberately
/// excludes the characters that terminate a URL in real writing — whitespace,
/// `)`, `]`, `>`, quotes, `,`, `;`, `!` — so an ordinary citation still matches.
fn continues_url(next: char) -> bool {
    next.is_ascii_alphanumeric()
        || matches!(
            next,
            '-' | '_' | '~' | '%' | '+' | '=' | '&' | '#' | '?' | ':' | '@'
        )
}

/// Whether `content` cites `url` itself, rather than merely opening with it.
///
/// A plain substring check would let a recorded `https://exa.ai/docs` claim
/// credit for a cited `https://exa.ai/docs-archive`, or a recorded
/// `https://exa.ai` for `https://exa.ai.evil.example` — neither of which the
/// search returned. So a match counts only where the next character cannot be
/// continuing the same URL ([`continues_url`]).
///
/// **Exact URLs only.** A deeper path under a recorded result
/// (`…/docs/reference` for a recorded `…/docs`) does *not* count: the search
/// returned the one page, and crediting a different page because it shares a
/// prefix is the false stamp this module refuses. The single exception is the
/// same URL written with a trailing slash, which is the same page.
fn cites(content: &str, url: &str) -> bool {
    let mut from = 0;
    while let Some(at) = content[from..].find(url) {
        let end = from + at + url.len();
        let mut rest = content[end..].chars();
        let terminated = match rest.next() {
            None => true,
            // `…/docs/` — the recorded page with a trailing slash, provided
            // nothing follows it. `…/docs/reference` is a different page.
            Some('/') => !rest.next().is_some_and(continues_url),
            // A `.` that begins a longer hostname (`exa.ai.evil.example`)
            // continues the URL; one that ends a sentence does not.
            Some('.') => !rest.next().is_some_and(continues_url),
            Some(next) => !continues_url(next),
        };
        if terminated {
            return true;
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
        // Re-attributing what we just produced is the round trip an agent makes
        // every time it reads a note back and writes it again.
        let once = p.attributed("Cites https://exa.ai/docs.").expect("footer");
        assert!(p.attributed(&once).is_none(), "{once}");
    }

    /// A document that *discusses* the footer has not been attributed by
    /// discussing it. Matching the phrase anywhere would let a style guide — or
    /// a brief about this very feature — suppress the credit it earned.
    #[test]
    fn a_passing_mention_of_the_phrase_does_not_count_as_attribution() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        let doc = "Our notes end with *Powered by Exa*, per https://exa.ai/docs.";
        let out = p
            .attributed(doc)
            .expect("mention must not suppress the footer");
        assert!(out.trim_end().ends_with(ATTRIBUTION_BLOCK), "{out}");
        assert_eq!(out.matches(ATTRIBUTION_BLOCK).count(), 1, "{out}");
    }

    /// The public line and the block that carries it must not drift apart.
    #[test]
    fn the_block_is_the_rule_plus_the_public_line() {
        assert!(ATTRIBUTION_BLOCK.ends_with(ATTRIBUTION_FOOTER));
        assert!(carries_attribution(&format!(
            "body\n\n{ATTRIBUTION_BLOCK}\n"
        )));
        assert!(!carries_attribution(
            "body mentioning *Powered by Exa* mid-sentence."
        ));
    }

    /// The same page written either way is the same citation; a *different*
    /// page under it is not. Search returned the one URL, and crediting a
    /// sibling because it shares a prefix is the false stamp this refuses.
    #[test]
    fn the_same_page_counts_and_a_deeper_one_does_not() {
        let p = provenance_with(&["https://exa.ai/docs/"]);
        assert!(p.cited_in("read https://exa.ai/docs today"));
        assert!(p.cited_in("read https://exa.ai/docs/ today"));
        assert!(p.cited_in("see [docs](https://exa.ai/docs)."));
        assert!(!p.cited_in("read https://exa.ai/docs/reference today"));
    }

    /// The prefix families that must not earn credit: a longer path segment, a
    /// longer host, a port, a query and a fragment — none of them were returned.
    #[test]
    fn a_url_that_merely_starts_with_a_recorded_one_earns_nothing() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        for impostor in [
            "https://exa.ai/docs-archive",
            "https://exa.ai/docs_v2",
            "https://exa.ai/docs%20old",
            "https://exa.ai/docs?utm=x",
            "https://exa.ai/docs#frag",
            "https://exa.ai/docsomething",
        ] {
            assert!(!p.cited_in(&format!("cited {impostor} here")), "{impostor}");
        }
        let host = provenance_with(&["https://exa.ai"]);
        assert!(!host.cited_in("https://exa.ai.evil.example/page"));
        assert!(!host.cited_in("https://exa.ai:8443/page"));
    }

    /// A query or fragment is part of the URL the backend returned, so
    /// normalization must not eat a slash out of one.
    #[test]
    fn normalization_strips_one_path_slash_and_never_touches_a_query() {
        let p = provenance_with(&["https://example.test/path/?next=/"]);
        assert!(p.cited_in("see https://example.test/path/?next=/ now"));
        // Only one slash, and only from a path.
        assert_eq!(
            normalize_url("https://example.test/a//").as_deref(),
            Some("https://example.test/a/")
        );
        assert_eq!(
            normalize_url("https://example.test/a#frag/").as_deref(),
            Some("https://example.test/a#frag/")
        );
    }

    /// The footer is the host's claim, not the model's. A body that arrives
    /// already stamped but cites nothing recorded has it removed — otherwise a
    /// model could mint the provenance claim by typing it, and a revision that
    /// drops its last citation would keep an attribution it no longer earns.
    #[test]
    fn an_unearned_footer_is_stripped_rather_than_trusted() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        let forged = format!("I wrote this myself.\n\n{ATTRIBUTION_BLOCK}\n");
        let cleaned = p
            .attributed(&forged)
            .expect("an unearned footer is removed");
        assert_eq!(cleaned, "I wrote this myself.");

        // A revision that drops the citation loses the footer with it.
        let earned = p.attributed("From https://exa.ai/docs.").expect("footer");
        assert!(earned.contains(ATTRIBUTION_FOOTER));
        let revised = earned.replace("https://exa.ai/docs", "our own notes");
        let cleaned = p.attributed(&revised).expect("no longer earned");
        assert!(!cleaned.contains(ATTRIBUTION_FOOTER), "{cleaned}");

        // With nothing recorded at all, a forged footer still does not stand.
        let empty = SearchProvenance::new();
        assert_eq!(
            empty.attributed(&forged).as_deref(),
            Some("I wrote this myself.")
        );
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
