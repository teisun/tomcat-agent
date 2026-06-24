import type { AskQuestionResult, WebviewTimelineItem } from "../types";
import { ApprovalCard } from "./ApprovalCard";
import { MessageBubble } from "./MessageBubble";
import { PlanFileCard } from "./PlanFileCard";
import { ThinkingBlock } from "./ThinkingBlock";
import { ToolCallCard } from "./ToolCallCard";

export function TranscriptView({
  busy,
  onAnswer,
  onApplyEdit,
  onOpenDiff,
  onOpenPlanFile,
  timeline,
}: {
  busy: boolean;
  onAnswer(requestId: string, result: AskQuestionResult): void;
  onApplyEdit(toolCallId: string): void;
  onOpenDiff(toolCallId: string): void;
  onOpenPlanFile(path: string): void;
  timeline: WebviewTimelineItem[];
}) {
  const lastThinkingId = busy
    ? [...timeline].reverse().find((item) => item.type === "thinking")?.id ?? null
    : null;

  return (
    <section className="tc-transcript" aria-label="active-session">
      {timeline.map((item) => {
        switch (item.type) {
          case "message":
            return <MessageBubble item={item} key={item.id} />;
          case "thinking":
            return (
              <ThinkingBlock
                isStreaming={item.id === lastThinkingId}
                item={item}
                key={item.id}
              />
            );
          case "tool":
            return (
              <ToolCallCard
                item={item}
                key={item.id}
                onApplyEdit={onApplyEdit}
                onOpenDiff={onOpenDiff}
              />
            );
          case "plan":
            return <PlanFileCard item={item} key={item.id} onOpenPlanFile={onOpenPlanFile} />;
          case "approval":
            return <ApprovalCard item={item} key={item.id} onAnswer={onAnswer} />;
        }
      })}
    </section>
  );
}
