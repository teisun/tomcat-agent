import type { WebviewTimelineItem } from "../types";
import { ApprovalCard } from "./ApprovalCard";
import { MessageBubble } from "./MessageBubble";
import { PlanFileCard } from "./PlanFileCard";
import { ThinkingBlock } from "./ThinkingBlock";
import { ToolCallCard } from "./ToolCallCard";

export function TranscriptView({
  onAnswer,
  onApplyEdit,
  onOpenDiff,
  onOpenPlanFile,
  timeline,
}: {
  onAnswer(
    requestId: string,
    questionId: string,
    optionId: string | null,
    pickedRecommended: boolean,
  ): void;
  onApplyEdit(toolCallId: string): void;
  onOpenDiff(toolCallId: string): void;
  onOpenPlanFile(path: string): void;
  timeline: WebviewTimelineItem[];
}) {
  return (
    <section className="tc-transcript" aria-label="active-session">
      {timeline.map((item) => {
        switch (item.type) {
          case "message":
            return <MessageBubble item={item} key={item.id} />;
          case "thinking":
            return <ThinkingBlock item={item} key={item.id} />;
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
