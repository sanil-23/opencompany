// The Session tab is a tab, an address, and one merged stream.
//
// Source-shape assertions, in the idiom this suite already uses for wiring that
// is invisible to a reviewer of a single diff (`assert-design-tokens.sh` is the
// precedent every one of these cites). Three things have to stay true and none
// of them shows up in a rendered snapshot:
//
//   1. the tab exists and is addressable, so `#/company/agent/<id>?tab=session`
//      lands (`#/team/<id>` rewrites onto that and drops the query);
//   2. the stream is fed by the host's own per-agent route, not by a
//      console-side merge of per-desk histories;
//   3. the agent-to-agent collapses are the room's, not second copies.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const detail = readFileSync("src/views/team/AgentDetailView.tsx", "utf8");
const session = readFileSync("src/views/team/AgentSession.tsx", "utf8");
const client = readFileSync("src/api/client.ts", "utf8");

describe("the Session tab", () => {
  /**
   * A tab is an address, not local state — the rule `task-detail-tab-address`
   * pins for the task page. `useHashTab` is what makes `#/team/<id>?tab=session`
   * resolve, so a link to a teammate's session survives a reload and can be
   * pasted to somebody else.
   */
  it("is registered in the tab table the hash resolves against", () => {
    expect(detail).toContain('{ id: "session", label: "Session"');
    expect(detail).toContain("useHashTab<AgentTab>(");
    expect(detail).toContain('AGENT_TABS.map((t) => t.id)');
  });

  it("renders the session panel under that id", () => {
    expect(detail).toMatch(
      /<PageTabPanel idBase="agent" id="session" value=\{tab\}>[\s\S]{0,400}<AgentSession/,
    );
  });

  /**
   * The stream must come from `GET {scope}/agents/{id}/session`.
   *
   * Which channels an agent can read is decided host-side by the same function
   * that decides the agent's own context. A console-side merge of per-desk
   * `chat/history` calls would be a second opinion about what a teammate can
   * see, and the two would drift — at which point the page starts claiming an
   * agent saw something it did not.
   */
  it("reads the host's own per-agent route rather than merging desks", () => {
    expect(client).toContain("/agents/${encodeURIComponent(agentId)}/session");
    expect(session).toContain("client.agentSession(agentId, company,");
    expect(session).not.toContain("getChatHistory");
  });

  /**
   * `fromHistory` is the room's mapping and is reused whole. It is what carries
   * `referralConversation` and `asideConversation` through untouched; a
   * hand-rolled map here would be a second answer to "what is a chat line".
   */
  it("maps rows through the room's own history mapping", () => {
    expect(session).toContain("fromHistory(rows)");
  });

  /**
   * The agent-to-agent collapses are imported from the room, not reimplemented.
   * An exchange between two teammates has to read the same way here as it does
   * in the channel it happened in.
   */
  it("reuses the room's referral and aside collapses", () => {
    expect(session).toContain('from "@/views/room/StepTimeline"');
    expect(session).toContain("<ReferralConversation crossing={message.referralConversation} />");
    expect(session).toContain("<AsideConversation aside={message.asideConversation} />");
    expect(session).toContain("<StepTimeline steps={message.steps} />");
  });

  /**
   * Every row says which channel it came from. Without it the stream is
   * unreadable: two teammates answering in two desks interleave with nothing to
   * tell them apart, which is the one thing merging the channels costs.
   */
  it("badges every row with the channel the host stamped", () => {
    expect(session).toContain("sessionChannel");
    expect(session).toContain('data-testid="agent-session-channel"');
  });

  /**
   * A host without the route is a host without this surface, not a failure. The
   * distinction matters because the honest message differs: "this host does not
   * keep one" invites no debugging, and a red error box does.
   */
  it("tells a 404 apart from a failed read", () => {
    expect(session).toContain('setLoad(status === 404 ? "unsupported" : "error")');
  });

  /**
   * The same stale-response guard `AgentRuns` carries (issue #1671): a read
   * started before a teammate switch must not commit its rows beneath the next
   * teammate's name.
   */
  it("discards a read that resolved after a teammate switch", () => {
    expect(session).toContain("generationRef");
    expect(session).toContain("if (generation !== generationRef.current) return;");
  });
});

/**
 * The raw view is the same stream with the rendering taken off: the turns as
 * the agent received them, each tool call unfolded.
 *
 * These are source-shape assertions for the same reason the ones above are —
 * the properties that matter here are about *which data* is rendered and *what
 * the address is*, and neither survives into a snapshot of the output.
 */
describe("the raw-turns toggle", () => {
  /**
   * An address, not local state, and for a sharper reason than the tab was: the
   * raw view is the thing you send to somebody. A link that lands on the chat
   * bubbles and asks the reader to go and find a switch has thrown away the
   * point of having been sent.
   */
  it("is addressable as ?raw", () => {
    expect(session).toContain('useHashFlag("raw")');
    expect(session).toContain('from "@/hooks/use-hash-flag"');
  });

  /**
   * Both states are named. A `Switch` labelled "Raw" names one state and leaves
   * the operator to infer the other, and "not raw" is not a thing with an
   * obvious name — so it is two buttons, the idiom the workflow index uses for
   * Cards/List, each carrying `aria-pressed`.
   */
  it("offers both views as named, pressable controls", () => {
    expect(session).toContain('data-testid="agent-session-view-chat"');
    expect(session).toContain('data-testid="agent-session-view-raw"');
    expect(session).toContain("aria-pressed={raw === value}");
  });

  /**
   * The raw view renders the **host's row**, not the mapped `ChatMessage`.
   *
   * `fromHistory` resolves `from` against the viewer, prefixes ids, and lifts
   * referrals and asides onto the bubble — every one of which is a rendering
   * decision somebody asking for the raw turns is asking to see past. Rendering
   * it from the mapped shape would make this a second opinion about the
   * transcript rather than the transcript.
   */
  it("renders the host's own row rather than the mapped message", () => {
    expect(session).toContain("row: AgentSessionMessageDto;");
    expect(session).toContain("<RawTurn key={line.row.id}");
    expect(session).toContain("{said ? row.text : cueLine(channel, row.author, row.text)}");
  });

  /**
   * A line somebody else said does not reach the agent as a bubble — it reaches
   * it as `[channel · author] text`, prepended to the turn by `render_cues` in
   * `src/harness/built_in/agent_session.rs`. Reproducing that literal shape is
   * the whole difference between this view and the chat one in a smaller font.
   */
  it("reproduces the host's cue shape for a line the agent was given", () => {
    expect(session).toContain("return `[${channel || \"?\"} · ${author}] ${text.trim()}`;");
  });

  /**
   * Which turns the agent *produced* comes from the channel the host journaled
   * the reply under (the agent's id), never from the author label — a label is
   * a display string and two teammates can share one.
   */
  it("tells its own turns apart by channel, not by author label", () => {
    expect(session).toContain("const said = row.channel === agentId;");
  });

  /**
   * Tool calls are unfolded, not named. The chat view's `StepTimeline` is a
   * summary, and a summary is what this view exists to get out from behind, so
   * the raw one prints each step's arguments and its result.
   */
  it("unfolds each step's arguments and result", () => {
    expect(session).toContain('data-testid="agent-session-raw-step"');
    expect(session).toContain("{step.detail}");
    expect(session).toContain("{step.result}");
    expect(session).toContain("step.truncated");
  });

  /**
   * The collapses do not survive into the raw view. A referral rendered as
   * "asked @copy · 2 msgs" is precisely the summary being opened up.
   */
  it("prints referral and aside lines in full rather than collapsed", () => {
    expect(session).toMatch(/row\.referralConversation\?\.lines\.map/);
    expect(session).toMatch(/row\.asideConversation\?\.lines\.map/);
  });
});
