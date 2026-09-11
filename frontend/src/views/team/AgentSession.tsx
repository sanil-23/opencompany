// One agent's whole session (the "one agent, one session" change).
//
// Every other tab on this page describes what a teammate *is* — its
// instructions, its toolbelt, its model — and `AgentRuns` says what it has
// *done*. This one says what it has **said and heard**: your DMs with it, its
// lines on every desk it sits on, the private asides it was party to, the
// questions it put to another desk, and the tool calls behind each answer, in
// the order it experienced them.
//
// # Why this is one stream and not a channel picker
//
// Because that is now what the agent itself has. Its in-memory history used to
// be cleared and re-seeded on every channel switch, so "the agent's session"
// was not a thing that existed — there were as many transcripts as there were
// desks, and no vantage point from which the teammate was one continuous
// participant. Splitting this view back into channels would be showing the old
// architecture on top of the new one.
//
// # Where the channel labels come from
//
// The host, not here. `GET {scope}/agents/{id}/session` stamps each row with
// the channel it was said on, resolved through the *same* function that decides
// which channels reach the agent's own context. A console-side merge of
// per-desk `chat/history` calls would be a second opinion about what a teammate
// can see, and the two would drift.
//
// # The operator sees more than the agent does
//
// Deliberately. Asides are narrowed for a peer agent and never for a person —
// privacy there is a deliberation device, not a security boundary — so this
// page shows every one in full. See `docs/spec/runtime/hivemind-asides.md`.

import { useCallback, useEffect, useRef, useState } from "react";
import { Braces, Loader2, MessageSquare, MessagesSquare } from "lucide-react";

import type { OpenCompanyClient } from "@/api/client";
import type { AgentSessionMessageDto, TurnStep } from "@/api/types";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useHashFlag } from "@/hooks/use-hash-flag";
import { fromHistory, type ChatMessage } from "@/lib/chat";
import { cn } from "@/lib/utils";
import {
  AsideConversation,
  ReferralConversation,
  StepTimeline,
} from "@/views/room/StepTimeline";

/** How many lines one page of the session carries. */
const SESSION_PAGE = 200;

/** A line plus the channel it was said on. */
interface SessionLine {
  message: ChatMessage;
  channel: string;
  channelId: string;
  /**
   * The host's own row, kept beside the mapped message because the raw view
   * renders **this** and not `message`.
   *
   * `fromHistory` is a rendering decision: it resolves `from` against the
   * viewer, prefixes ids, and lifts referrals and asides onto the bubble. All
   * of that is exactly what somebody asking for the raw turns is asking to see
   * past. Rendering the raw view from the mapped shape would make it a second
   * opinion about the transcript rather than the transcript.
   */
  row: AgentSessionMessageDto;
}

type Load = "loading" | "ready" | "unsupported" | "error";

export function AgentSession({
  client,
  company,
  agentId,
  agentName,
}: {
  client: OpenCompanyClient;
  company: string | null;
  agentId: string;
  agentName: string;
}) {
  const [lines, setLines] = useState<SessionLine[]>([]);
  const [load, setLoad] = useState<Load>("loading");
  /**
   * Whether to show the turns as the agent received them rather than as chat.
   *
   * An address (`?tab=session&raw`), not local state, for the reason the Edit
   * form is one: the raw view is the thing you send to somebody else. "Look at
   * what it actually saw" is a link, and a link that lands on the rendered
   * bubbles and asks the reader to find a switch has lost the point of being
   * sent. `useHashFlag` also makes Back close it, which is the behaviour a
   * reader who opened it out of curiosity expects.
   */
  const [raw, setRaw] = useHashFlag("raw");
  // Incremented whenever the fetch effect restarts. A read that started before
  // a teammate switch captures the old value and discards its answer, so rows
  // fetched for one teammate can never be committed beneath another's name —
  // the same guard `AgentRuns` carries, for the same reason (issue #1671).
  const generationRef = useRef(0);

  const read = useCallback(async () => {
    const generation = ++generationRef.current;
    setLoad("loading");
    try {
      const rows: AgentSessionMessageDto[] = await client.agentSession(agentId, company, {
        limit: SESSION_PAGE,
      });
      if (generation !== generationRef.current) return;
      // `fromHistory` is the room's own mapping, reused whole: it is what
      // prefixes host ids, resolves `from`, and carries `referralConversation`
      // and `asideConversation` through untouched. Mapping these rows by hand
      // would be a second answer to "what is a chat line" that would drift from
      // the room's.
      const mapped = fromHistory(rows);
      setLines(
        mapped.flatMap((message, index) => {
          const row = rows[index];
          if (!row) return [];
          return [
            {
              message,
              channel: row.sessionChannel ?? "",
              channelId: row.sessionChannelId ?? "",
              row,
            },
          ];
        }),
      );
      setLoad("ready");
    } catch (error) {
      if (generation !== generationRef.current) return;
      // A host that predates the route is not an error to shout about — it is a
      // host without this surface, and saying so is more useful than a red box
      // that implies something broke.
      const status = (error as { status?: number } | null)?.status;
      setLoad(status === 404 ? "unsupported" : "error");
    }
  }, [client, company, agentId]);

  useEffect(() => {
    void read();
  }, [read]);

  if (load === "loading") {
    return (
      <Card>
        <CardContent className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="size-4 animate-spin" aria-hidden />
          Reading {agentName}&apos;s session…
        </CardContent>
      </Card>
    );
  }

  if (load === "unsupported") {
    return (
      <Card>
        <CardContent className="text-sm text-muted-foreground">
          This host does not keep a per-agent session yet, so there is nothing to
          show here. Each desk&apos;s own transcript is still on the Room page.
        </CardContent>
      </Card>
    );
  }

  if (load === "error") {
    return (
      <Card>
        <CardContent className="text-sm text-muted-foreground">
          {agentName}&apos;s session could not be read. It is still there — this
          is a failed request, not an empty history.
        </CardContent>
      </Card>
    );
  }

  if (lines.length === 0) {
    return (
      <Card>
        <CardContent className="flex items-center gap-2 text-sm text-muted-foreground">
          <MessageSquare className="size-4 shrink-0" aria-hidden />
          {agentName} has not said or heard anything yet.
        </CardContent>
      </Card>
    );
  }

  return (
    <Card>
      <CardContent className="space-y-4">
        <div className="flex items-center justify-between gap-3">
          <p className="text-xs text-muted-foreground">
            {raw
              ? "Every turn as the agent received it, in order, with each tool call unfolded."
              : "Everything this teammate has said and heard, across every channel it can read."}
          </p>
          <ViewToggle raw={raw} onChange={setRaw} />
        </div>
        {raw ? (
          <ol className="space-y-3" data-testid="agent-session-raw">
            {lines.map((line) => (
              <RawTurn key={line.row.id} line={line} agentId={agentId} />
            ))}
          </ol>
        ) : (
          <ol className="space-y-4" data-testid="agent-session">
            {lines.map((line) => (
              <SessionRow key={line.message.id} line={line} agentId={agentId} />
            ))}
          </ol>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * Chat or raw turns.
 *
 * Two buttons rather than a `Switch`, in the idiom the workflow index already
 * uses for Cards/List: a switch names one state and leaves the operator to
 * infer the other, and "Raw" is not a thing that is obviously on or off.
 */
function ViewToggle({
  raw,
  onChange,
}: {
  raw: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-1 rounded-lg border p-0.5">
      {(
        [
          { value: false, label: "Chat", Icon: MessagesSquare, id: "chat" },
          { value: true, label: "Raw turns", Icon: Braces, id: "raw" },
        ] as const
      ).map(({ value, label, Icon, id }) => (
        <Button
          key={id}
          size="sm"
          variant={raw === value ? "secondary" : "ghost"}
          className="h-7 px-2"
          onClick={() => onChange(value)}
          aria-pressed={raw === value}
          data-testid={`agent-session-view-${id}`}
        >
          <Icon className="mr-1.5 size-3.5" aria-hidden />
          {label}
        </Button>
      ))}
    </div>
  );
}

/** One line of the session, badged with where it was said. */
function SessionRow({ line, agentId }: { line: SessionLine; agentId: string }) {
  const { message, channel } = line;
  // Who is speaking, from the reader's point of view. `from` is resolved
  // host-side against the *viewer*, so "you" here means the operator reading
  // the page — which is why the agent's own lines are matched by author rather
  // than by `from`.
  const mine = message.from === "you";
  const author = mine ? "You" : (message.channel ?? agentId);

  return (
    <li className="flex gap-3" data-testid="agent-session-row">
      <TeammateAvatar
        name={author}
        className="mt-0.5 size-6 shrink-0"
        markOnly
      />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-semibold">{author}</span>
          {channel && (
            <Badge
              variant="outline"
              className={cn("text-2xs font-normal text-muted-foreground")}
              data-testid="agent-session-channel"
            >
              {channel}
            </Badge>
          )}
        </div>
        <p className="text-sm leading-relaxed whitespace-pre-wrap">{message.text}</p>
        {/* The room's own collapses, reused rather than reimplemented — an
            agent-to-agent exchange has to read the same way here as it does in
            the channel it happened in, or the two surfaces disagree about what
            was said. */}
        {!!message.steps?.length && <StepTimeline steps={message.steps} />}
        {message.referralConversation && (
          <ReferralConversation crossing={message.referralConversation} />
        )}
        {message.asideConversation && (
          <AsideConversation aside={message.asideConversation} />
        )}
      </div>
    </li>
  );
}

/**
 * One turn, as the agent received it.
 *
 * # What "raw" means here, exactly
 *
 * It means the journal row, unrendered — not a dump of the model's context
 * window, which is process-local, bounded by `max_history_messages`, and gone
 * the moment the host restarts. The session an operator can be shown is the one
 * the host rebuilds from the journal on every turn, and that is what this is.
 *
 * The one thing it reproduces literally is the **cue line**. A line somebody
 * else said does not reach the agent as a chat bubble; it reaches it as
 * `[channel · author] text`, prepended to the turn — `render_cues` in
 * `src/harness/built_in/agent_session.rs`. Showing it in that shape is the
 * difference between "here is the transcript again, in a smaller font" and
 * "here is the string the model was handed".
 *
 * Nothing collapses. The referral and aside collapses the chat view reuses from
 * the room are summaries, and a summary is the thing this view exists to get
 * out from behind — so they render as their own lines, in full.
 */
function RawTurn({ line, agentId }: { line: SessionLine; agentId: string }) {
  const { row, channel } = line;
  // An agent reply is journaled with the agent's id as its channel
  // (`chat_history.rs`), which is what separates the turns this teammate
  // *produced* from the ones it was *given*. Never inferred from the author
  // label: a label is a display string and two teammates may share one.
  const said = row.channel === agentId;
  const at = new Date(row.atMillis);

  return (
    <li
      className="rounded-md border bg-muted/30 p-2.5"
      data-testid="agent-session-raw-turn"
      data-direction={said ? "said" : "heard"}
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 font-mono text-2xs text-muted-foreground">
        <span>#{row.id}</span>
        <span>{at.toLocaleTimeString()}</span>
        <span className={cn(said ? "text-foreground" : undefined)}>
          {said ? "said" : "heard"}
        </span>
        {channel && (
          <Badge
            variant="outline"
            className="text-2xs font-normal"
            data-testid="agent-session-raw-channel"
          >
            {channel}
          </Badge>
        )}
        {row.taskId && <span>task {row.taskId}</span>}
        {row.parentId && <span>reply to #{row.parentId}</span>}
      </div>
      <pre className="mt-1.5 font-mono text-xs leading-relaxed whitespace-pre-wrap">
        {said ? row.text : cueLine(channel, row.author, row.text)}
      </pre>
      {!!row.steps?.length && (
        <ol className="mt-2 space-y-1.5 border-t pt-2" data-testid="agent-session-raw-steps">
          {row.steps.map((step, index) => (
            <RawStep key={`${row.id}:${index}`} step={step} />
          ))}
        </ol>
      )}
      {row.referralConversation?.lines.map((referral, index) => (
        <pre
          key={`referral:${index}`}
          className="mt-1 font-mono text-2xs leading-relaxed whitespace-pre-wrap text-muted-foreground"
        >
          {cueLine(
            referral.outbound ? "referral out" : "referral in",
            referral.authorLabel || referral.authorId,
            referral.text,
          )}
        </pre>
      ))}
      {row.asideConversation?.lines.map((aside, index) => (
        <pre
          key={`aside:${index}`}
          className="mt-1 font-mono text-2xs leading-relaxed whitespace-pre-wrap text-muted-foreground"
        >
          {cueLine("aside", aside.authorId, aside.text)}
        </pre>
      ))}
    </li>
  );
}

/**
 * The host's own cue shape, reproduced.
 *
 * Kept as one function so the two places this view renders an inbound line —
 * the turn body and the referral/aside lines under it — cannot disagree about
 * what a cue looks like. `render_cues` trims the text; so does this.
 */
function cueLine(channel: string, author: string, text: string): string {
  return `[${channel || "?"} · ${author}] ${text.trim()}`;
}

/** One tool call, with its arguments and its result unfolded rather than named. */
function RawStep({ step }: { step: TurnStep }) {
  return (
    <li className="font-mono text-2xs leading-relaxed" data-testid="agent-session-raw-step">
      <div className="flex flex-wrap items-center gap-x-2">
        <span className="font-semibold">{step.label}</span>
        <span className="text-muted-foreground">{step.kind}</span>
        <span
          className={cn(
            step.status === "error" ? "text-destructive" : "text-muted-foreground",
          )}
        >
          {step.status}
        </span>
        {step.elapsedMs != null && (
          <span className="text-muted-foreground">{step.elapsedMs}ms</span>
        )}
        {step.failure && <span className="text-destructive">{step.failure}</span>}
      </div>
      {step.detail && (
        <pre className="whitespace-pre-wrap text-muted-foreground">→ {step.detail}</pre>
      )}
      {step.result && (
        <pre className="whitespace-pre-wrap text-muted-foreground">
          ← {step.result}
          {step.truncated ? " (truncated before the agent read all of it)" : ""}
        </pre>
      )}
    </li>
  );
}
