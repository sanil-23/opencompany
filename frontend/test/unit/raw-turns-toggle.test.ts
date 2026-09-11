// Raw turns: one renderer, reachable from both places you would ask for it.
//
// Source-shape assertions, in the idiom this suite uses for wiring no rendered
// snapshot can show (`assert-design-tokens.sh` is the precedent every one of
// these cites). Four things have to stay true:
//
//   1. there is ONE renderer, shared — "what the agent saw" is a claim about
//      the runtime, and a claim that reads differently depending on which
//      screen you are on is two claims;
//   2. it renders the host's row, not the console's mapped `ChatMessage`;
//   3. it is reachable from the DM, which is where the question gets asked;
//   4. both entry points are addresses, so the answer can be sent to somebody.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const raw = readFileSync("src/views/room/RawTurns.tsx", "utf8");
const session = readFileSync("src/views/team/AgentSession.tsx", "utf8");
const header = readFileSync("src/views/room/ChatHeader.tsx", "utf8");
const room = readFileSync("src/views/RoomView.tsx", "utf8");

describe("the raw-turns renderer", () => {
  /**
   * Both surfaces import it. A second copy in the room would drift from the
   * one on the teammate's page, and the two would then disagree about what a
   * teammate was handed — which is the only thing this view is for.
   */
  it("is one component both surfaces import", () => {
    expect(session).toContain('from "@/views/room/RawTurns"');
    expect(room).toContain('from "./room/RawTurns"');
    expect(raw).toContain("export function RawTurns(");
  });

  /**
   * It takes the **host's row**, not the mapped `ChatMessage`.
   *
   * `fromHistory` resolves the speaker against the viewer, prefixes ids and
   * lifts collapses onto the bubble — every one of which is a rendering
   * decision a reader asking for the raw turns is asking to see past. Feeding
   * it the mapped shape would make this a second opinion about the transcript
   * rather than the transcript.
   */
  it("takes the host's own rows", () => {
    expect(raw).toContain("rows: AgentSessionMessageDto[];");
    expect(session).toContain("rows={lines.map((line) => line.row)}");
  });

  /**
   * A line somebody else said does not reach the agent as a bubble — it reaches
   * it as `[channel · author] text`, prepended to the turn by `render_cues` in
   * `src/harness/built_in/agent_session.rs`. Reproducing that literal shape is
   * the whole difference between this and the chat view in a smaller font.
   */
  it("reproduces the host's cue shape for a line the agent was given", () => {
    expect(raw).toContain("return `[${channel || \"?\"} · ${author}] ${text.trim()}`;");
  });

  /**
   * The author half of the cue comes from `cueAuthor` — the string the host
   * built the cue with — never from `author`.
   *
   * The two resolve differently on purpose. An agent's byline becomes a
   * per-line attribution prefix, so it is a stable id that cannot be chosen and
   * therefore cannot be chosen to impersonate; a person reading a transcript
   * gets the display ladder instead, which ends at "someone". Rendering
   * `author` in the cue would put a name in front of an operator that the agent
   * never saw — the one thing this view exists not to do.
   */
  it("takes the cue's author from the host, not from the display name", () => {
    expect(raw).toContain(
      "{said ? row.text : cueLine(channel, row.cueAuthor ?? row.author, row.text)}",
    );
    expect(raw).toContain("row.cueAuthor && row.cueAuthor !== row.author");
  });

  /**
   * Which turns the agent *produced* comes from the channel the host journaled
   * the reply under (the agent's id), never from the author label — a label is
   * a display string and two teammates can share one.
   */
  it("tells its own turns apart by channel, not by author label", () => {
    expect(raw).toContain("const said = row.channel === agentId;");
    expect(raw).toContain('data-direction={said ? "said" : "heard"}');
  });

  /**
   * Tool calls are unfolded, not named, and the collapses do not survive: a
   * referral rendered as "asked @copy · 2 msgs" is precisely the summary this
   * view exists to open up.
   */
  it("unfolds steps and prints referral and aside lines in full", () => {
    expect(raw).toContain('data-testid="agent-session-raw-step"');
    expect(raw).toContain("{step.detail}");
    expect(raw).toContain("{step.result}");
    expect(raw).toContain("step.truncated");
    expect(raw).toMatch(/row\.referralConversation\?\.lines\.map/);
    expect(raw).toMatch(/row\.asideConversation\?\.lines\.map/);
  });

  /**
   * Channel badges are on for the whole-session stream, where rows from several
   * desks interleave, and off inside one DM, where every badge would repeat the
   * same word down the page.
   */
  it("badges the channel only where rows come from more than one", () => {
    expect(raw).toContain("showChannel = false");
    expect(session).toContain("showChannel");
    expect(room).toContain("<RawTurns rows={rows} agentId={agentId} />");
  });
});

describe("the raw-turns toggle in a DM", () => {
  /**
   * The control is in the chat header, because that is where you are when the
   * question occurs to you. "Why did it answer that" is asked mid-conversation,
   * and a control for it that lives two navigations away is one nobody finds.
   */
  it("is a control on the chat header", () => {
    expect(header).toContain('data-testid="chat-raw-toggle"');
    expect(header).toContain("aria-pressed={raw}");
    expect(header).toContain("<span>Raw turns</span>");
    expect(room).toContain("rawAvailable={!!rawAgentId}");
    expect(room).toContain("onToggleRaw={() => setRawRequested(!showRaw)}");
  });

  /**
   * Only in a DM. A `#channel` has several agents and the Operator feed has
   * none, so "the raw turns" would have to pick one for you — which is worse
   * than not offering it. Same rule the member pane follows for that feed.
   */
  it("is offered only where exactly one teammate is on the other end", () => {
    expect(room).toContain(
      'const rawAgentId = channel?.kind === "dm" ? (channel.member?.id ?? null) : null;',
    );
    expect(header).toContain("rawAvailable && onToggleRaw");
  });

  /**
   * An address (`#/chat/<id>?raw`), like the Session tab's. `useHashView`
   * strips everything from `?` onward before resolving a segment, so it rides
   * the chat route without the router seeing it.
   */
  it("is addressable", () => {
    expect(room).toContain('useHashFlag("raw")');
    expect(session).toContain('useHashFlag("raw")');
  });

  /**
   * Scoped to the conversation on screen. A toggle changes how the thing in
   * front of you is drawn; it must not change what the thing is, and flipping
   * a DM into a stream that also carries `#general` would do that. The
   * cross-channel view keeps its own address, and the pane links to it.
   */
  it("shows this conversation's turns, and links to the whole session", () => {
    expect(room).toContain("rows.filter((row) => inDmWith(row, rawAgentId))");
    expect(room).toContain("?tab=session&raw");
  });

  /**
   * Both DM spellings. `agent_channels` registers a teammate's DM under its
   * bare id (what `dmThreadId` posts to, after #364 re-keyed DMs) *and* under
   * `dm:<id>`. Matching one would silently drop every line keyed the other way.
   */
  it("matches both spellings of a DM channel key", () => {
    expect(room).toContain("row.sessionChannelId === agentId || row.sessionChannelId === `dm:${agentId}`");
  });

  /**
   * The same stale-response guard the Session tab carries (issue #1671): a read
   * started before the operator switched DMs must not commit its rows beneath
   * the next teammate's name.
   */
  it("discards a read that resolved after a DM switch", () => {
    expect(room).toContain("rawGenerationRef");
    expect(room).toContain("if (generation !== rawGenerationRef.current) return;");
  });

  /**
   * A host without the per-agent route is a host without this surface, not a
   * failure — the honest message differs, and a red box invites debugging that
   * has nothing to find.
   */
  it("tells a 404 apart from a failed read", () => {
    expect(room).toContain('setRawLoad(status === 404 ? "unsupported" : "error")');
  });
});
