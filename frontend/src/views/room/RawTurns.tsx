// The turns an agent actually took, unrendered.
//
// # Why this lives in `room/` and is shared
//
// Two surfaces show it: the DM you have open with a teammate, and the Session
// tab on that teammate's page. They must not be two renderings — "what the
// agent saw" is a claim about the runtime, and a claim that reads differently
// depending on which screen you happen to be on is two claims. Same direction
// of reuse as `StepTimeline`, which the team views already import from here.
//
// # What "raw" means here, exactly
//
// The journal row, unrendered — **not** a dump of the model's context window.
// That is process-local, bounded by `max_history_messages`, and gone the moment
// the host restarts. The session an operator can be shown is the one the host
// rebuilds from the journal on every turn, and that is what this is.
//
// The one thing it reproduces literally is the **cue line**. A line somebody
// else said does not reach the agent as a chat bubble; it reaches it as
// `[channel · author] text`, prepended to the turn — `render_cues` in
// `src/harness/built_in/agent_session.rs`. Showing it in that shape is the
// difference between "here is the transcript again, in a smaller font" and
// "here is the string the model was handed".
//
// Nothing collapses. The referral and aside collapses the chat views reuse from
// `StepTimeline` are summaries, and a summary is the thing this exists to get
// out from behind, so they render as their own lines, in full.

import type { AgentSessionMessageDto, TurnStep } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

export function RawTurns({
  rows,
  agentId,
  showChannel = false,
}: {
  rows: AgentSessionMessageDto[];
  /**
   * Whose session this is. Decides which rows read as `said`, so it is required
   * rather than inferred — a stream with no owner has no direction.
   */
  agentId: string;
  /**
   * Whether to badge each turn with its channel. On for the whole-session view,
   * where rows from several desks interleave; off inside one conversation,
   * where every badge would say the same word.
   */
  showChannel?: boolean;
}) {
  return (
    <ol className="space-y-3" data-testid="agent-session-raw">
      {rows.map((row) => (
        <RawTurn
          key={row.id}
          row={row}
          agentId={agentId}
          showChannel={showChannel}
        />
      ))}
    </ol>
  );
}

/** One turn, as the agent received it. */
function RawTurn({
  row,
  agentId,
  showChannel,
}: {
  row: AgentSessionMessageDto;
  agentId: string;
  showChannel: boolean;
}) {
  const channel = row.sessionChannel ?? "";
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
        {showChannel && channel && (
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
        <ol
          className="mt-2 space-y-1.5 border-t pt-2"
          data-testid="agent-session-raw-steps"
        >
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
 * Kept as one function so the two places a turn renders an inbound line — the
 * turn body and the referral/aside lines under it — cannot disagree about what
 * a cue looks like. `render_cues` trims the text; so does this.
 */
function cueLine(channel: string, author: string, text: string): string {
  return `[${channel || "?"} · ${author}] ${text.trim()}`;
}

/** One tool call, with its arguments and its result unfolded rather than named. */
function RawStep({ step }: { step: TurnStep }) {
  return (
    <li
      className="font-mono text-2xs leading-relaxed"
      data-testid="agent-session-raw-step"
    >
      <div className="flex flex-wrap items-center gap-x-2">
        <span className="font-semibold">{step.label}</span>
        <span className="text-muted-foreground">{step.kind}</span>
        <span
          className={cn(
            step.status === "error"
              ? "text-destructive"
              : "text-muted-foreground",
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
        <pre className="whitespace-pre-wrap text-muted-foreground">
          → {step.detail}
        </pre>
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
