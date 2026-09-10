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
import { Loader2, MessageSquare } from "lucide-react";

import type { OpenCompanyClient } from "@/api/client";
import type { AgentSessionMessageDto } from "@/api/types";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
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
        mapped.map((message, index) => ({
          message,
          channel: rows[index]?.sessionChannel ?? "",
          channelId: rows[index]?.sessionChannelId ?? "",
        })),
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
        <ol className="space-y-4" data-testid="agent-session">
          {lines.map((line) => (
            <SessionRow key={line.message.id} line={line} agentId={agentId} />
          ))}
        </ol>
      </CardContent>
    </Card>
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
