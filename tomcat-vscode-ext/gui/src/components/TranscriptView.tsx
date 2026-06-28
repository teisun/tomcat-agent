import { Fragment, type RefObject } from "react";

import { selectActiveTodoSource } from "../hooks/useActiveTodoProgress";
import type {
  AskQuestionResult,
  WebviewPlanState,
  WebviewTimelineItem,
  WebviewTodo,
} from "../types";
import { ApprovalCard } from "./ApprovalCard";
import { BoundaryBlock } from "./BoundaryBlock";
import { MessageBubble } from "./MessageBubble";
import { PlanFileCard } from "./PlanFileCard";
import { ProgressRow } from "./ProgressRow";
import { ThinkingBlock } from "./ThinkingBlock";
import { ThinkingGroup } from "./ThinkingGroup";
import { ToolRow } from "./ToolRow";
import {
  groupTimelineByAssistantResponse,
  type AssistantResponseGroup,
  type GroupedTimelineEntry,
} from "./sessionList/groupTimelineByAssistantResponse";

export function TranscriptView({
  busy,
  bottomSpacerHeight = 0,
  canBuildPlan,
  onAnswer,
  onBuildPlan,
  onOpenFile,
  onOpenPlanFile,
  planState,
  planTodos = [],
  sessionTodos = [],
  timeline,
  transcriptRef,
}: {
  busy: boolean;
  bottomSpacerHeight?: number;
  canBuildPlan: boolean;
  onAnswer(requestId: string, result: AskQuestionResult): void;
  onBuildPlan(planId: string | null, path: string): void;
  onOpenFile(path: string): void;
  onOpenPlanFile(path: string): void;
  planState?: WebviewPlanState | null;
  planTodos?: WebviewTodo[];
  sessionTodos?: WebviewTodo[];
  timeline: WebviewTimelineItem[];
  transcriptRef?: RefObject<HTMLElement | null>;
}) {
  const lastThinkingId = busy
    ? [...timeline].reverse().find((item) => item.type === "thinking")?.id ?? null
    : null;
  const latestUserIndex = timeline.reduce(
    (lastIndex, item, index) =>
      item.type === "message" && item.kind === "user" ? index : lastIndex,
    -1,
  );

  const renderGroupedItem = (item: GroupedTimelineEntry) => {
    if ("type" in item && item.type === "assistant-response-group") {
      const group = item as AssistantResponseGroup;
      const hasMeaningfulThinking = Boolean(group.thinking?.text.trim());
      if (group.tools.length === 1 && !hasMeaningfulThinking) {
        return (
          <Fragment key={`group-tool-${group.assistantMessageId}`}>
            {group.preamble ? <MessageBubble item={group.preamble} key={`${group.preamble.id}-solo`} /> : null}
            <ToolRow
              item={group.tools[0]}
              key={`group-tool-row-${group.tools[0].id}`}
              onOpenFile={onOpenFile}
            />
          </Fragment>
        );
      }
      const isStreaming =
        busy &&
        (group.thinking?.id === lastThinkingId ||
          group.tools.some((tool) => tool.status !== "complete"));
      return (
        <ThinkingGroup
          group={group}
          isStreaming={isStreaming}
          key={`group-${group.assistantMessageId}`}
          onOpenFile={onOpenFile}
        />
      );
    }

    return renderTimelineItem(item as WebviewTimelineItem);
  };

  const renderTimelineItem = (item: WebviewTimelineItem) => {
    switch (item.type) {
      case "boundary":
        return <BoundaryBlock item={item} key={item.id} />;
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
          <ToolRow
            item={item}
            key={item.id}
            onOpenFile={onOpenFile}
          />
        );
      case "plan":
        return (
          <PlanFileCard
            canBuild={canBuildPlan}
            item={item}
            key={item.id}
            onBuild={onBuildPlan}
            onOpenPlanFile={onOpenPlanFile}
            planTodos={planTodos}
          />
        );
      case "approval":
        return <ApprovalCard item={item} key={item.id} onAnswer={onAnswer} />;
    }
  };

  const renderCluster = (clusterTimeline: WebviewTimelineItem[], showProgress: boolean) => {
    const grouped = groupTimelineByAssistantResponse(clusterTimeline);
    const hasActiveThinking = clusterTimeline.some((item) => item.type === "thinking");
    const hasRunningTool = clusterTimeline.some(
      (item) => item.type === "tool" && item.status !== "complete",
    );
    const hasStreamingText = clusterTimeline.some(
      (item) => item.type === "message" && item.kind === "assistant",
    );
    const hasTodos = Boolean(
      selectActiveTodoSource({
        busy,
        planState,
        planTodos,
        sessionTodos,
      })?.length,
    );
    return (
      <>
        {grouped.map(renderGroupedItem)}
        {showProgress ? (
          <ProgressRow
            busy={busy}
            hasActiveThinking={hasActiveThinking}
            hasRunningTool={hasRunningTool}
            hasStreamingText={hasStreamingText}
            hasTodos={hasTodos}
          />
        ) : null}
      </>
    );
  };

  const leadingTimeline =
    busy && latestUserIndex >= 0 ? timeline.slice(0, latestUserIndex + 1) : timeline;
  const liveClusterTimeline =
    busy && latestUserIndex >= 0 ? timeline.slice(latestUserIndex + 1) : [];

  return (
    <section
      className="tc-transcript"
      aria-label="active-session"
      ref={transcriptRef}
    >
      {renderCluster(leadingTimeline, false)}
      {liveClusterTimeline.length ? (
        <div className="tc-live-cluster" data-testid="live-cluster">
          {renderCluster(liveClusterTimeline, true)}
        </div>
      ) : null}
      <div
        aria-hidden="true"
        className="tc-transcript__spacer"
        data-testid="transcript-spacer"
        style={{ height: `${bottomSpacerHeight}px` }}
      />
    </section>
  );
}
