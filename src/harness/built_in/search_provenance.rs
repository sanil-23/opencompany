//! Search-result provenance for workspace attribution.
//!
//! The managed `web_search` tool ([`search`](super::search)) hands an agent
//! third-party sources it is expected to cite. When that agent then puts a
//! document into the company workspace, the operator reading it has no way to
//! tell whether it was grounded in those searched sources or written from the
//! model's memory. This module closes that gap with an attribution footer —
//! but only on **evidence**, never on a timer:
//!
//! * [`SearchProvenance`] is one **per-company** record of the result URLs the
//!   managed search tool returned, held on
//!   [`SearchBackend`](crate::harness::search::SearchBackend) beside the daily
//!   call ledger. Company-scoped because the question is whether *this
//!   company's* search returned the cited URL, not whether the writing agent
//!   personally ran it — a per-agent record let one teammate strip the footer
//!   another teammate's document had legitimately earned.
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
//!
//! Matching parses candidate URLs out of the body and compares them whole
//! ([`cited_urls`]), rather than searching for each recorded URL and judging
//! its neighbours. Every character that ends a URL in prose (`,`, `;`, `!`,
//! `'`) is also legal inside one, so a boundary allowlist must either credit
//! `…/a$rev` for a recorded `…/a` or refuse an ordinary "see …/a, and".
//!
//! Attribution is **additive**: the host appends its own claim and never
//! removes text a model or an operator wrote. The record is in-process and
//! bounded, so a missing URL is not proof a footer was unearned — a restart or
//! an eviction would otherwise delete true credits on a schedule. Policing a
//! supplied footer needs durable per-document evidence, which belongs in the
//! storage ports rather than in a heuristic over the body.

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
        cited_urls(content)
            .iter()
            .any(|candidate| tracked.contains(candidate))
    }

    /// `content` with the attribution footer appended, when it has earned one:
    /// it cites a recorded URL and does not already carry the footer. `None`
    /// means "store the body exactly as given".
    ///
    /// **Additive only.** An earlier revision removed a footer that cited
    /// nothing recorded, on the reasoning that the claim is the host's to make
    /// rather than the model's. That cannot be done safely from this record:
    /// it is in-process like the daily ledger and bounded at
    /// [`MAX_TRACKED_URLS`], so a restart or an eviction leaves a genuinely
    /// attributed document looking unearned, and the strip would delete a true
    /// credit. The two errors are not symmetric — an over-stamped draft is
    /// cosmetic, while erasing real attributions happens on every restart — so
    /// the host adds its own claim and never removes text somebody else wrote.
    /// Policing a supplied footer needs durable per-document evidence, which is
    /// a storage-port change rather than a heuristic over the body.
    pub fn attributed(&self, content: &str) -> Option<String> {
        if carries_attribution(content) || !self.cited_in(content) {
            return None;
        }
        Some(format!(
            "{body}\n\n{ATTRIBUTION_BLOCK}\n",
            body = content.trim_end()
        ))
    }
}

/// Whether `content` already ends with the attribution block, and so must be
/// stored as given rather than footered twice.
///
/// Anchored to the end after trimming trailing whitespace: that is where
/// [`SearchProvenance::attributed`] puts it, and matching anywhere would let a
/// passing mention of the phrase suppress a footer the document has earned.
pub fn carries_attribution(content: &str) -> bool {
    let trimmed = content.trim_end();
    // The block must also START a line. `Note---\n*Powered by Exa*` ends with
    // the same characters but contains no horizontal rule, and treating it as a
    // footer would suppress the real one on a document that earned it.
    match trimmed.strip_suffix(ATTRIBUTION_BLOCK) {
        Some(before) => before.is_empty() || before.ends_with('\n'),
        None => false,
    }
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

/// Characters RFC 3986 permits in a URI after the scheme, beside alphanumerics.
const URL_CHARS: &str = "-._~:/?#[]@!$&'()*+,;=%";

/// Trailing characters that end a sentence rather than a URL.
///
/// `)` and `]` are URL-legal, so they are dropped only when unbalanced — which
/// is what makes `[docs](https://exa.ai/docs)` and `(https://exa.ai/docs)` read
/// correctly without truncating a URL that genuinely contains a bracket.
fn trim_trailing_punctuation(url: &str) -> &str {
    let mut url = url;
    loop {
        let last = match url.chars().next_back() {
            Some(c) => c,
            None => return url,
        };
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => url.matches(')').count() > url.matches('(').count(),
            ']' => url.matches(']').count() > url.matches('[').count(),
            _ => false,
        };
        if !drop {
            return url;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
}

/// Every absolute `http`/`https` URL written in `content`, normalized.
/// text for each recorded URL and inspecting its neighbours. Boundary-guessing
/// cannot get this right: every character a heuristic must treat as "ends the
/// URL" (`,`, `;`, `!`, `'`, `*`, `$`) is also legal *inside* one, so any
/// allowlist either credits `…/a$rev` for a recorded `…/a` or refuses an
/// ordinary "see …/a, and". Reading the URL out and comparing it for equality
/// has no such tension, and it makes a wrapper like
/// `https://tracker.test/?next=https://exa.ai/docs` one candidate — the
/// wrapper, which was never returned — instead of two.
fn cited_urls(content: &str) -> Vec<String> {
    let lower = content.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("://") {
        let sep = from + at;
        // Walk back over the WHOLE scheme. A scheme is `ALPHA *( ALPHA / DIGIT
        // / "+" / "-" / "." )` (RFC 3986), so the `-` and `.` are scheme
        // characters, not separators: `git-https://…`, `git+https://…` and
        // `git.https://…` each name a *different* URI whose inner `https` is
        // not a citation of the recorded page. Anything but a plain
        // `http`/`https` is skipped whole.
        //
        // Walk characters, not bytes: `rfind`'s byte offset points at the start
        // of the character that ends the scheme, so adding 1 lands inside a
        // multi-byte one whenever a cited URL follows non-ASCII punctuation
        // (a curly quote, an em dash, a full-width colon). Walking
        // `char_indices` keeps every index a boundary, so the slice below
        // cannot panic.
        let start = content[..sep]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            .map(|(i, _)| i)
            .last()
            .unwrap_or(sep);
        // Walk forward over everything a URI may contain — needed by BOTH
        // branches, so it runs before the scheme check.
        let mut end = sep + 3;
        for (offset, c) in content[sep + 3..].char_indices() {
            if c.is_ascii_alphanumeric() || URL_CHARS.contains(c) {
                end = sep + 3 + offset + c.len_utf8();
            } else {
                break;
            }
        }
        if !matches!(&lower[start..sep], "http" | "https") {
            // Consume the WHOLE URI, not just past its `://`. A rejected outer
            // scheme (`ftp`, `git-https`, …) may carry a nested `https://…` in
            // its path or query — `ftp://proxy.test/?next=https://exa.ai/docs`
            // — and that value is a parameter of the wrapper, never a citation
            // of its own. Resuming at `sep + 3` would re-scan it as a fresh
            // candidate and credit the inner page.
            from = end.max(sep + 3);
            continue;
        }
        let mut candidate_start = start;
        let autolink = start > 0 && content.as_bytes()[start - 1] == b'<';
        if autolink {
            candidate_start -= 1;
        }
        let candidate = &content[candidate_start..end];
        let candidate = if autolink {
            candidate.strip_suffix('>').unwrap_or(candidate)
        } else {
            candidate
        };
        // A Markdown link destination — `]` immediately before the `(` —
        // keeps its URL-legal punctuation. `[source](https://example.test/a!)`
        // cites the URL ending in `!`; the closing `)` is the link terminator
        // (the LAST one, so a balanced pair inside the URL survives), and
        // everything after it is prose the forward walk swallowed. Trimming the
        // `!` too would collapse `…/a!` onto a recorded `…/a` and stamp the
        // wrong page. Bare prose parens (`(see https://exa.ai/a!)`) stay on the
        // ordinary trim path, where the `!` is sentence punctuation.
        let markdown_dest = start >= 2
            && content.as_bytes()[start - 1] == b'('
            && content.as_bytes()[start - 2] == b']';
        let candidate = if autolink {
            candidate.strip_prefix('<').unwrap_or(candidate)
        } else if markdown_dest {
            match candidate.rfind(')') {
                Some(at) => &candidate[..at],
                None => candidate,
            }
        } else {
            trim_trailing_punctuation(candidate)
        };
        if let Some(url) = normalize_url(candidate) {
            found.push(url);
        }
        // Continue past the whole candidate, so a URL carried inside another as
        // a parameter is never counted as a citation of its own.
        from = end.max(sep + 3);
    }
    found
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

    /// Attribution is additive: the host appends its own claim and never
    /// deletes text somebody else wrote. Policing a supplied footer would mean
    /// deleting a true credit every time the in-process record resets or
    /// evicts, which is the worse of the two errors.
    #[test]
    fn a_supplied_footer_is_left_in_place_rather_than_deleted() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        let forged = format!("I wrote this myself.\n\n{ATTRIBUTION_BLOCK}\n");
        assert!(p.attributed(&forged).is_none(), "nothing is ever removed");

        // Including after a reset or an eviction, which is the case that made
        // stripping unsound: the citation is intact, the record simply no
        // longer holds the URL.
        let reset = provenance_with(&["https://unrelated.test/page"]);
        let genuine = format!("From https://exa.ai/docs.\n\n{ATTRIBUTION_BLOCK}\n");
        assert!(reset.attributed(&genuine).is_none(), "{genuine}");
        assert!(SearchProvenance::new().attributed(&genuine).is_none());
    }

    /// Every sub-delimiter is legal inside a path, so a longer URL that merely
    /// begins with a recorded one earns nothing — the case a character
    /// allowlist could not express, since the same characters end URLs in prose.
    #[test]
    fn a_sub_delimiter_suffix_is_a_different_url() {
        let p = provenance_with(&["https://example.test/a"]);
        for suffix in [
            "$rev", "!v2", "'x", "*star", ",list", ";p=1", "+plus", "=eq",
        ] {
            let cited = format!("https://example.test/a{suffix}");
            assert!(!p.cited_in(&format!("see {cited} here")), "{cited}");
        }
        // …while the same characters as sentence punctuation still terminate a
        // genuine citation.
        for prose in [
            "see https://example.test/a, then",
            "see https://example.test/a; then",
            "see https://example.test/a! Really",
            "quoted 'https://example.test/a'",
            "see https://example.test/a.",
        ] {
            assert!(p.cited_in(prose), "{prose}");
        }
    }

    /// A wrapper URL carrying a recorded one as a parameter was never returned
    /// by the search, so it earns nothing — the leading edge matters as much as
    /// the trailing one.
    #[test]
    fn a_recorded_url_embedded_in_a_larger_one_earns_nothing() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        for wrapper in [
            "https://tracker.test/?next=https://exa.ai/docs",
            "https://proxy.test/https://exa.ai/docs",
            "https://cache.test/x.https://exa.ai/docs",
        ] {
            assert!(!p.cited_in(&format!("go via {wrapper} today")), "{wrapper}");
        }
        // The genuine citation still matches in ordinary prose shapes.
        for prose in [
            "see https://exa.ai/docs",
            "see (https://exa.ai/docs)",
            "see [docs](https://exa.ai/docs).",
            "https://exa.ai/docs",
        ] {
            assert!(p.cited_in(prose), "{prose}");
        }
    }

    /// A trailing slash is the same page only when the recorded URL is bare:
    /// `…?next=x` and `…?next=x/` are different query values.
    #[test]
    fn the_trailing_slash_equivalence_is_for_paths_only() {
        let bare = provenance_with(&["https://example.test/path"]);
        assert!(bare.cited_in("at https://example.test/path/ now"));

        let query = provenance_with(&["https://example.test/path?next=x"]);
        assert!(query.cited_in("at https://example.test/path?next=x now"));
        assert!(!query.cited_in("at https://example.test/path?next=x/ now"));

        let fragment = provenance_with(&["https://example.test/path#top"]);
        assert!(!fragment.cited_in("at https://example.test/path#top/ now"));
    }

    /// `Note---` is not a horizontal rule, so a body ending that way has not
    /// been attributed and must still earn its footer.
    #[test]
    fn the_block_must_begin_its_own_line() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        let sneaky = format!("Cites https://exa.ai/docs.\nNote{ATTRIBUTION_BLOCK}\n");
        assert!(!carries_attribution(&sneaky));
        let out = p.attributed(&sneaky).expect("must still be attributed");
        assert!(carries_attribution(&out), "{out}");
    }

    /// The record is company-scoped, so a teammate who did not run the search
    /// still verifies a colleague's citation rather than stripping it.
    #[test]
    fn one_record_serves_every_agent_of_the_company() {
        let shared = SearchProvenance::new();
        // The researcher searches…
        shared.record(["https://exa.ai/docs"]);
        // …and a different agent, holding the same handle, writes the note.
        let note = shared
            .attributed("Per https://exa.ai/docs, the API is POST.")
            .expect("a teammate's citation is still evidence");
        assert!(carries_attribution(&note));
        // Re-writing it later keeps the footer rather than stripping it.
        assert!(shared.attributed(&note).is_none());
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

    #[test]
    fn a_longer_host_gets_no_credit() {
        let p = provenance_with(&["https://exa.ai"]);
        assert!(!p.cited_in("see https://exa.ai.evil.example/page"));
        assert!(!p.cited_in("see https://exa.aique.example/page"));
        // …while genuine end-of-sentence punctuation does.
        assert!(p.cited_in("see https://exa.ai. Next sentence."));
        assert!(p.cited_in("see (https://exa.ai)"));
    }

    /// A multi-byte character immediately before the scheme (a curly quote, an
    /// em dash, a full-width colon) must not break the boundary walk: the old
    /// byte offset plus one landed inside the character and sliced
    /// `&lower[start..sep]` at a non-boundary, panicking the tool call. The
    /// whole turn is the failure mode, not a missed footer.
    #[test]
    fn a_non_ascii_character_before_the_url_does_not_panic() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        for prose in [
            "Per “https://exa.ai/docs”, the API is POST.",
            "Source—https://exa.ai/docs",
            "来源：https://exa.ai/docs",
            "Документ: https://exa.ai/docs.",
        ] {
            let out = p.attributed(prose).expect("footer expected");
            assert!(carries_attribution(&out), "{prose}");
        }
    }

    /// `-`, `.` and `+` are scheme characters (RFC 3986), so a longer scheme
    /// that merely *contains* `https` is a different URI, not a citation of the
    /// recorded page. The walk-back must consume the whole scheme or the inner
    /// `https://…` would be read as a citation for a `git-https`/`git.https`
    /// document that never touched a search result.
    #[test]
    fn an_outer_scheme_with_a_separator_is_not_the_inner_url() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        for wrapper in [
            "git-https://exa.ai/docs",
            "git+https://exa.ai/docs",
            "git.https://exa.ai/docs",
            "sub.https://exa.ai/docs",
            "1https://exa.ai/docs",
        ] {
            assert!(!p.cited_in(&format!("via {wrapper} today")), "{wrapper}");
        }
        // The plain https citation still counts in the same prose.
        assert!(p.cited_in("via https://exa.ai/docs today"));
    }

    /// A rejected outer scheme is consumed WHOLE, so a nested `https://` in its
    /// path or query is never re-scanned as a citation of its own.
    /// `ftp://proxy.test/?next=https://exa.ai/docs` cites only the FTP URI —
    /// the inner value is a parameter of the wrapper, exactly like the
    /// accepted-scheme wrappers above.
    #[test]
    fn a_nested_url_inside_a_rejected_scheme_is_not_a_citation() {
        let p = provenance_with(&["https://exa.ai/docs"]);
        for wrapper in [
            "ftp://proxy.test/?next=https://exa.ai/docs",
            "ftp://proxy.test/https://exa.ai/docs",
        ] {
            assert!(!p.cited_in(&format!("download via {wrapper} now")), "{wrapper}");
        }
        // The genuine citation still matches in the same prose.
        assert!(p.cited_in("download via https://exa.ai/docs now"));
    }

    /// A Markdown link destination keeps its URL-legal punctuation: `!` is a
    /// sub-delim, so `[source](https://example.test/a!)` cites the URL ending in
    /// `!` — never the recorded `https://example.test/a`. Only the closing `)`
    /// (the link terminator) is stripped, and only prose after it.
    #[test]
    fn a_markdown_link_destination_preserves_url_legal_punctuation() {
        let p = provenance_with(&["https://example.test/a"]);
        for doc in [
            "See [the source](https://example.test/a!) for the prices.",
            "Per [the source](https://example.test/a!), the price is fixed.",
        ] {
            assert!(!p.cited_in(doc), "{doc} cites a different URL");
        }
        // The identical URL in a Markdown destination still matches.
        let q = provenance_with(&["https://example.test/a!"]);
        assert!(q.cited_in("See [the source](https://example.test/a!)."));
        // …while the same `!` outside a destination stays sentence punctuation.
        assert!(p.cited_in("see https://example.test/a!"));
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
