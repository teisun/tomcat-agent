import { Fragment, type RefObject, useMemo } from "react";

import { selectActiveTodoSource } from "../hooks/useActiveTodoProgress";
import type {
  AskQuestionResult,
  WebviewCheckpoint,
  WebviewPlanFileCard,
  WebviewPlanState,
  WebviewTimelineItem,
  WebviewTodo,
  WebviewToolCard,
} from "../types";
import { ApprovalCard } from "./ApprovalCard";
import { BoundaryBlock } from "./BoundaryBlock";
import { CheckpointMarker } from "./CheckpointMarker";
import { injectCheckpointMarkers } from "./checkpointMarkers";
import { MessageBubble } from "./MessageBubble";
import { PlanFileCard } from "./PlanFileCard";
import { ProgressRow } from "./ProgressRow";
import { ThinkingBlock } from "./ThinkingBlock";
import { ThinkingGroup } from "./ThinkingGroup";
import { isActionTool, isSuppressedPlanToolRow, ToolRow } from "./ToolRow";
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

function asString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function isPlanWorkflowTool(tool: WebviewToolCard): boolean {
  return (
    tool.toolName === "create_plan" ||
    tool.toolName === "update_plan" ||
    tool.display?.kind === "plan"
  );
}

function isRunningPlanTool(item: WebviewTimelineItem): item is WebviewToolCard {
  return (
    item.type === "tool" &&
    !item.isError &&
    (item.status === "running" || item.status === "streaming") &&
    isPlanWorkflowTool(item)
  );
}

function toolPlanId(tool: WebviewToolCard): string | null {
  return asString(tool.args?.plan_id) ?? asString(tool.args?.planId);
}

function toolPlanPath(tool: WebviewToolCard): string | null {
  return tool.display?.kind === "plan" ? tool.display.plan : asString(tool.args?.path);
}

function matchesPlanCard(tool: WebviewToolCard, item: WebviewPlanFileCard): boolean {
  const candidatePlanId = toolPlanId(tool);
  if (candidatePlanId && item.planId && candidatePlanId === item.planId) {
    return true;
  }
  const candidatePath = toolPlanPath(tool);
  return candidatePath === item.path;
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
    const activePlanTools = clusterTimeline.filter(isRunningPlanTool);
    const planCards = clusterTimeline.filter(
      (item): item is WebviewPlanFileCard => item.type === "plan",
    );
    const latestPlanCardId = [...planCards].reverse()[0]?.id ?? null;
    const matchedPlanCardIds = new Set(
      planCards
        .filter((item) => activePlanTools.some((tool) => matchesPlanCard(tool, item)))
        .map((item) => item.id),
    );
    const shouldFallbackToLatestPlanCard =
      activePlanTools.length > 0 && matchedPlanCardIds.size === 0;

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
          if (isSuppressedPlanToolRow(item)) {
            return null;
          }
          return (
            <ToolRow
              item={item}
              key={item.id}
              onOpenDiff={onOpenDiff}
              onOpenFile={onOpenFile}
            />
          );
        case "plan":
          return (
            <PlanFileCard
              availableModels={availableModels}
              buildModel={buildModel}
              canBuild={canBuildPlan}
              creating={
                matchedPlanCardIds.has(item.id) ||
                (shouldFallbackToLatestPlanCard && latestPlanCardId === item.id)
              }
              item={item}
              key={item.id}
              onBuild={onBuildPlan}
              onOpenPlanFile={onOpenPlanFile}
              onSetBuildModel={onSetBuildModel}
              planTodos={planTodos}
            />
          );
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
                    item={segment.tool}
                    key={`group-action-${segment.tool.id}`}
                    onOpenDiff={onOpenDiff}
                    onOpenFile={onOpenFile}
                  />
                );
              }
              const hasThinkingText = Boolean(segment.group.thinking?.text.trim());
              const renderableTools = segment.group.tools.filter(
                (tool) => !isSuppressedPlanToolRow(tool),
              );
              if (renderableTools.length === 0 && !hasThinkingText) {
                return null;
              }
              if (renderableTools.length === 1 && !hasThinkingText) {
                return (
                  <ToolRow
                    item={renderableTools[0]}
                    key={`group-context-standalone-${renderableTools[0].id}`}
                    onOpenDiff={onOpenDiff}
                    onOpenFile={onOpenFile}
                  />
                );
              }
              const isStreaming =
                showProgress &&
                (segment.group.thinking?.id === clusterLastThinkingId ||
                  segment.group.tools.some((tool) => tool.status !== "complete"));
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
    busy && latestUserIndex >= 0
      ? renderedTimeline.slice(0, latestUserIndex + 1)
      : renderedTimeline;
  const liveClusterTimeline =
    busy && latestUserIndex >= 0 ? renderedTimeline.slice(latestUserIndex + 1) : [];

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
