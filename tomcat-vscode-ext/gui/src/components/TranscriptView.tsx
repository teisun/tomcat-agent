import { Fragment, type RefObject, useMemo } from "react";

import { selectActiveTodoSource } from "../hooks/useActiveTodoProgress";
import type {
  AskQuestionResult,
  WebviewCheckpoint,
  WebviewPlanState,
  WebviewTimelineItem,
  WebviewTodo,
} from "../types";
import { ApprovalCard } from "./ApprovalCard";
import { BoundaryBlock } from "./BoundaryBlock";
import { CheckpointMarker } from "./CheckpointMarker";
import { injectCheckpointMarkers } from "./checkpointMarkers";
import { MessageBubble } from "./MessageBubble";
import { ProgressRow } from "./ProgressRow";
import { ThinkingBlock } from "./ThinkingBlock";
import { ThinkingGroup } from "./ThinkingGroup";
import { isActionTool, ToolRow } from "./ToolRow";
import {
  groupTimelineByAssistantResponse,
  type AssistantResponseGroup,
  type GroupedTimelineEntry,
} from "./sessionList/groupTimelineByAssistantResponse";

export type AssistantRenderEntry =
  | {
      group: AssistantResponseGroup;
      type: "context-group";
    }
  | {
      tool: Extract<WebviewTimelineItem, { type: "tool" }>;
      type: "action-tool";
    };

function lastLiveActivityType(
  clusterTimeline: WebviewTimelineItem[],
): "message" | "thinking" | "tool" | null {
  for (let index = clusterTimeline.length - 1; index >= 0; index -= 1) {
    const item = clusterTimeline[index];
    if (item.type === "thinking" || item.type === "tool") {
      return item.type;
    }
    if (item.type === "message" && item.kind === "assistant") {
      return "message";
    }
  }
  return null;
}

export function partitionAssistantResponseGroup(
  group: AssistantResponseGroup,
): AssistantRenderEntry[] {
  const entries: AssistantRenderEntry[] = [];
  const bufferedTools: AssistantResponseGroup["tools"] = [];
  let thinkingConsumed = false;

  const flushContext = () => {
    const includeThinking =
      !thinkingConsumed &&
      !!group.thinking &&
      (bufferedTools.length > 0 || Boolean(group.thinking.text.trim()));
    if (!includeThinking && bufferedTools.length === 0) {
      return;
    }
    entries.push({
      group: {
        assistantMessageId: group.assistantMessageId,
        thinking: includeThinking ? group.thinking : undefined,
        tools: [...bufferedTools],
        type: "assistant-response-group",
      },
      type: "context-group",
    });
    bufferedTools.length = 0;
    thinkingConsumed = thinkingConsumed || includeThinking;
  };

  if (group.tools.length === 0) {
    flushContext();
    return entries;
  }

  for (const tool of group.tools) {
    if (isActionTool(tool)) {
      flushContext();
      entries.push({
        tool,
        type: "action-tool",
      });
      continue;
    }
    bufferedTools.push(tool);
  }

  flushContext();
  return entries;
}

export function TranscriptView({
  availableModels = [],
  buildModel = "",
  busy,
  bottomSpacerHeight = 0,
  canBuildPlan,
  checkpoints = [],
  onAnswer,
  onBuildPlan,
  onOpenDiff,
  onOpenFile,
  onOpenPlanFile,
  onRestoreCheckpoint,
  onRetryUserMessage,
  onSetBuildModel,
  planId,
  planState,
  planTodos = [],
  sessionTodos = [],
  timeline,
  transcriptRef,
}: {
  availableModels?: string[];
  buildModel?: string;
  busy: boolean;
  bottomSpacerHeight?: number;
  canBuildPlan: boolean;
  checkpoints?: WebviewCheckpoint[];
  onAnswer(requestId: string, result: AskQuestionResult): void;
  onBuildPlan(planId: string | null, path: string): void;
  onOpenDiff?(toolCallId: string): void;
  onOpenFile(path: string, line?: number): void;
  onOpenPlanFile(path: string): void;
  onRestoreCheckpoint?(checkpointId: string): void;
  onRetryUserMessage?(messageId: string): void;
  onSetBuildModel?(modelId: string): void;
  planId?: string | null;
  planState?: WebviewPlanState | null;
  planTodos?: WebviewTodo[];
  sessionTodos?: WebviewTodo[];
  timeline: WebviewTimelineItem[];
  transcriptRef?: RefObject<HTMLElement | null>;
}) {
  const renderedTimeline = useMemo(
    () => injectCheckpointMarkers(timeline, checkpoints),
    [checkpoints, timeline],
  );
  const latestUserIndex = renderedTimeline.reduce(
    (lastIndex, item, index) =>
      item.type === "message" && item.kind === "user" ? index : lastIndex,
    -1,
  );

  const renderCluster = (clusterTimeline: WebviewTimelineItem[], showProgress: boolean) => {
    const grouped = groupTimelineByAssistantResponse(clusterTimeline);
    const clusterLastThinkingId = showProgress
      ? [...clusterTimeline].reverse().find((item) => item.type === "thinking")?.id ?? null
      : null;
    const lastLiveActivity = showProgress ? lastLiveActivityType(clusterTimeline) : null;

    const renderTimelineItem = (item: WebviewTimelineItem) => {
      switch (item.type) {
        case "boundary":
          return <BoundaryBlock item={item} key={item.id} />;
        case "message":
          return (
            <MessageBubble
              item={item}
              key={item.id}
              onOpenFile={onOpenFile}
              onRetry={onRetryUserMessage}
            />
          );
        case "checkpoint":
          return (
            <CheckpointMarker
              item={item}
              key={item.id}
              onRestore={(checkpoint) => onRestoreCheckpoint?.(checkpoint.checkpointId)}
            />
          );
        case "thinking":
          return (
            <ThinkingBlock
              isStreaming={showProgress && item.id === clusterLastThinkingId}
              item={item}
              key={item.id}
              onOpenFile={onOpenFile}
            />
          );
        case "tool":
          return (
            <ToolRow
              availableModels={availableModels}
              buildModel={buildModel}
              canBuildPlan={canBuildPlan}
              currentPlanId={planId}
              currentPlanState={planState}
              item={item}
              key={item.id}
              onBuildPlan={onBuildPlan}
              onOpenDiff={onOpenDiff}
              onOpenFile={onOpenFile}
              onOpenPlanFile={onOpenPlanFile}
              onSetBuildModel={onSetBuildModel}
              planTodos={planTodos}
            />
          );
        case "plan":
          return null;
        case "approval":
          return <ApprovalCard item={item} key={item.id} onAnswer={onAnswer} />;
      }
    };

    const renderGroupedItem = (item: GroupedTimelineEntry) => {
      if ("type" in item && item.type === "assistant-response-group") {
        const group = item as AssistantResponseGroup;
        const segments = partitionAssistantResponseGroup(group);
        return (
          <Fragment key={`group-${group.assistantMessageId}`}>
            {group.preamble ? (
              <MessageBubble
                item={group.preamble}
                key={`${group.preamble.id}-preamble`}
                onOpenFile={onOpenFile}
                onRetry={onRetryUserMessage}
              />
            ) : null}
            {segments.map((segment, index) => {
              if (segment.type === "action-tool") {
                return (
                  <ToolRow
                    availableModels={availableModels}
                    buildModel={buildModel}
                    canBuildPlan={canBuildPlan}
                    currentPlanId={planId}
                    currentPlanState={planState}
                    item={segment.tool}
                    key={`group-action-${segment.tool.id}`}
                    onBuildPlan={onBuildPlan}
                    onOpenDiff={onOpenDiff}
                    onOpenFile={onOpenFile}
                    onOpenPlanFile={onOpenPlanFile}
                    onSetBuildModel={onSetBuildModel}
                    planTodos={planTodos}
                  />
                );
              }
              const hasThinkingText = Boolean(segment.group.thinking?.text.trim());
              if (segment.group.tools.length === 0 && !hasThinkingText) {
                return null;
              }
              if (segment.group.tools.length === 1 && !hasThinkingText) {
                return (
                  <ToolRow
                    item={segment.group.tools[0]}
                    key={`group-context-standalone-${segment.group.tools[0].id}`}
                    onOpenDiff={onOpenDiff}
                    onOpenFile={onOpenFile}
                  />
                );
              }
              const hasIncompleteTools = segment.group.tools.some(
                (tool) => tool.status !== "complete",
              );
              const isStreaming =
                showProgress &&
                (hasIncompleteTools ||
                  (segment.group.tools.length === 0 &&
                    segment.group.thinking?.id === clusterLastThinkingId));
              return (
                <ThinkingGroup
                  group={segment.group}
                  isStreaming={isStreaming}
                  key={`group-context-${group.assistantMessageId}-${index}`}
                  onOpenDiff={onOpenDiff}
                  onOpenFile={onOpenFile}
                />
              );
            })}
          </Fragment>
        );
      }

      return renderTimelineItem(item as WebviewTimelineItem);
    };

    const hasActiveThinking = lastLiveActivity === "thinking";
    const hasRunningTool = clusterTimeline.some(
      (item) => item.type === "tool" && item.status !== "complete",
    );
    const hasStreamingText = lastLiveActivity === "message";
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
    busy && latestUserIndex >= 0
      ? renderedTimeline.slice(0, latestUserIndex + 1)
      : renderedTimeline;
  const showLiveCluster = busy && latestUserIndex >= 0;
  const liveClusterTimeline = showLiveCluster
    ? renderedTimeline.slice(latestUserIndex + 1)
    : [];

  return (
    <section
      className="tc-transcript"
      aria-label="active-session"
      ref={transcriptRef}
    >
      {renderCluster(leadingTimeline, false)}
      {showLiveCluster ? (
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
