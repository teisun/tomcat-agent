import { Fragment, type RefObject, type ReactNode, useMemo } from "react";

import type {
  AskQuestionResult,
  PathResolution,
  WebviewCheckpoint,
  WebviewMediaRoot,
  WebviewPlanFileState,
  WebviewTimelineItem,
  WebviewTodo,
} from "../types";
import {
  ApprovalCard,
  approvalAnswerKey,
  type ApprovalAnswerDraft,
  type ApprovalAnswerState,
} from "./ApprovalCard";
import { BoundaryBlock } from "./BoundaryBlock";
import { CheckpointMarker } from "./CheckpointMarker";
import { messageIdsWithWritesAfter, injectCheckpointMarkers } from "./checkpointMarkers";
import { MessageBubble } from "./MessageBubble";
import type { Speed } from "../../../src/shared/modelSpeed";
import type { ModelPickerModel } from "./ModelPicker";
import { createPlanFileCardFromTool, PlanFileCard } from "./PlanFileCard";
import { ProgressRow } from "./ProgressRow";
import { ReviewRow } from "./ReviewRow";
import { ThinkingBlock } from "./ThinkingBlock";
import { ThinkingGroup } from "./ThinkingGroup";
import { isActionTool, ToolRow, toolCategory } from "./ToolRow";
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

function assistantMessageIdForLiveItem(item: WebviewTimelineItem): string | null {
  if (item.type === "tool" || item.type === "thinking") {
    return item.assistantMessageId ?? null;
  }
  if (item.type === "message" && item.kind === "assistant") {
    return item.assistantMessageId ?? null;
  }
  return null;
}

function lastLiveAssistantMessageId(
  clusterTimeline: WebviewTimelineItem[],
): string | null {
  for (let index = clusterTimeline.length - 1; index >= 0; index -= 1) {
    const assistantMessageId = assistantMessageIdForLiveItem(clusterTimeline[index]);
    if (assistantMessageId) {
      return assistantMessageId;
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
  renderUserEditor,
  onEditUserMessage,
  approvalAnswers = {},
  availableModelDetails,
  availableModels = [],
  buildModel = "",
  busy,
  bottomSpacerHeight = 0,
  canBuildPlan,
  checkpoints = [],
  onAnswer,
  onApprovalDraftChange,
  onBuildPlan,
  onOpenDiff,
  onOpenFile,
  onOpenLink,
  onOpenImagePreview,
  onOpenPlanFile,
  onRecoverErrorTurn,
  onRestoreCheckpoint,
  onRetryUserMessage,
  onSelectContextWindow,
  onSelectThinkingLevel,
  onSelectSpeed,
  onSetBuildModel,
  onZoomImage,
  resolvePaths,
  planId,
  planState,
  planTodos = [],
  sessionContextWindow,
  sessionModel = "",
  sessionThinkingLevel,
  sessionTodos = [],
  timeline,
  mediaRoots,
  transcriptRef,
}: {
  renderUserEditor?(message: Extract<WebviewTimelineItem, {type:"message"}>): ReactNode;
  onEditUserMessage?(message: Extract<WebviewTimelineItem, {type:"message"}>): void;
  approvalAnswers?: Record<string, ApprovalAnswerState>;
  availableModelDetails?: Record<string, ModelPickerModel>;
  availableModels?: string[];
  buildModel?: string;
  busy: boolean;
  bottomSpacerHeight?: number;
  canBuildPlan: boolean;
  checkpoints?: WebviewCheckpoint[];
  onAnswer(sessionId: string, requestId: string, result: AskQuestionResult): void;
  onApprovalDraftChange(
    sessionId: string,
    requestId: string,
    draft: ApprovalAnswerDraft,
  ): void;
  onBuildPlan(planId: string | null, path: string): void;
  onOpenDiff?(toolCallId: string): void;
  onOpenFile(path: string, line?: number): void;
  onOpenLink?(href: string): void;
  onOpenImagePreview?(imageId: string): void;
  onOpenPlanFile(path: string): void;
  onRecoverErrorTurn?(errorId: string, action: "resume" | "retry"): void;
  onRestoreCheckpoint?(checkpointId: string): void;
  onRetryUserMessage?(messageId: string): void;
  onSelectContextWindow(modelId: string, contextWindow: number): void;
  onSelectThinkingLevel(modelId: string, level: string): void;
  onSelectSpeed(modelId: string, speed: Speed): void;
  onSetBuildModel(modelId: string): void;
  onZoomImage?(image: { alt: string; src: string }): void;
  resolvePaths?: (paths: string[]) => Promise<PathResolution[]>;
  planId?: string | null;
  planState?: WebviewPlanFileState | null;
  planTodos?: WebviewTodo[];
  sessionContextWindow?: number | null;
  sessionModel?: string;
  sessionThinkingLevel?: string | null;
  sessionTodos?: WebviewTodo[];
  timeline: WebviewTimelineItem[];
  mediaRoots?: WebviewMediaRoot[];
  transcriptRef?: RefObject<HTMLElement | null>;
}) {
  const messagesWithWrites = useMemo(() => messageIdsWithWritesAfter(timeline), [timeline]);
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
    const activeAssistantMessageId = showProgress
      ? lastLiveAssistantMessageId(clusterTimeline)
      : null;
    const planModelPicker = {
      availableModelDetails,
      availableModels,
      buildModel,
      onSelectContextWindow,
      onSelectThinkingLevel,
      onSelectSpeed,
      onSetBuildModel,
      sessionContextWindow,
      sessionModel,
      sessionThinkingLevel,
    };

    const renderTool = (
      item: Extract<WebviewTimelineItem, { type: "tool" }>,
      key: string,
    ) => {
      const planCard = createPlanFileCardFromTool(item, {
        currentPlanId: planId,
        currentPlanState: planState,
        planTodos,
      });
      if (planCard) {
        return (
          <PlanFileCard
            canBuild={canBuildPlan}
            creating={item.status === "running" || item.status === "streaming"}
            item={planCard}
            key={key}
            modelPicker={planModelPicker}
            onBuild={onBuildPlan}
            onOpenPlanFile={onOpenPlanFile}
            planTodos={planTodos}
          />
        );
      }
      return (
        <ToolRow
          item={item}
          key={key}
          onOpenDiff={onOpenDiff}
          onOpenFile={onOpenFile}
          onOpenPlanFile={onOpenPlanFile}
        />
      );
    };

    const renderTimelineItem = (item: WebviewTimelineItem) => {
      switch (item.type) {
        case "boundary":
          return <BoundaryBlock item={item} key={item.id} />;
        case "message":
          if (item.kind === "user") {
            const editor = renderUserEditor?.(item);
            if (editor) return <Fragment key={item.id}>{editor}</Fragment>;
          }
          return (
            <MessageBubble
              item={item}
              onEdit={item.kind === "user" && item.rewindEligible ? onEditUserMessage : undefined}
              hasFileWrites={item.kind === "user" ? messagesWithWrites.has(item.id) : undefined}
              key={item.id}
              mediaRoots={mediaRoots}
              onOpenFile={onOpenFile}
              onOpenLink={onOpenLink}
              onOpenImagePreview={onOpenImagePreview}
              onRecover={onRecoverErrorTurn}
              onRetry={onRetryUserMessage}
              recoveryDisabled={busy}
              resolvePaths={resolvePaths}
              onZoomImage={onZoomImage}
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
          return renderTool(item, item.id);
        case "plan":
          return null;
        case "review":
          return <ReviewRow item={item} key={item.id} />;
        case "approval": {
          const answerState = approvalAnswers[
            approvalAnswerKey(item.sessionId ?? "", item.request.requestId)
          ];
          return (
            <ApprovalCard
              draft={answerState?.draft}
              item={item}
              key={item.id}
              onAnswer={onAnswer}
              onDraftChange={onApprovalDraftChange}
              submitting={answerState?.submitting}
            />
          );
        }
      }
    };

    const renderGroupedItem = (item: GroupedTimelineEntry) => {
      if ("type" in item && item.type === "assistant-response-group") {
        const group = item as AssistantResponseGroup;
        const segments = partitionAssistantResponseGroup(group);
        const isActiveGroup =
          activeAssistantMessageId !== null &&
          group.assistantMessageId === activeAssistantMessageId;
        const lastContextGroupIndex = isActiveGroup
          ? segments.reduce(
              (lastIndex, segment, index) =>
                segment.type === "context-group" ? index : lastIndex,
              -1,
            )
          : -1;
        return (
          <Fragment key={`group-${group.assistantMessageId}`}>
            {group.preamble ? (
              <MessageBubble
                item={group.preamble}
                key={`${group.preamble.id}-preamble`}
                mediaRoots={mediaRoots}
                onOpenFile={onOpenFile}
                onOpenImagePreview={onOpenImagePreview}
                onRecover={onRecoverErrorTurn}
                onRetry={onRetryUserMessage}
                recoveryDisabled={busy}
                onZoomImage={onZoomImage}
              />
            ) : null}
            {segments.map((segment, index) => {
              const isActiveTailContextGroup = isActiveGroup && index === lastContextGroupIndex;
              if (segment.type === "action-tool") {
                return renderTool(segment.tool, `group-action-${segment.tool.id}`);
              }
              const hasThinkingText = Boolean(segment.group.thinking?.text.trim());
              if (segment.group.tools.length === 0 && !hasThinkingText) {
                return null;
              }
              if (
                segment.group.tools.length === 1 &&
                !hasThinkingText &&
                toolCategory(segment.group.tools[0].toolName) !== "task" &&
                !isActiveTailContextGroup
              ) {
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
                  isLive={isActiveTailContextGroup}
                  isStreaming={isStreaming}
                  key={`group-context-${group.assistantMessageId}-${index}`}
                  mediaRoots={mediaRoots}
                  onOpenDiff={onOpenDiff}
                  onOpenFile={onOpenFile}
                  onZoomImage={onZoomImage}
                />
              );
            })}
          </Fragment>
        );
      }

      return renderTimelineItem(item as WebviewTimelineItem);
    };
    return (
      <>
        {grouped.map(renderGroupedItem)}
        {showProgress ? <ProgressRow busy={showProgress} /> : null}
      </>
    );
  };

  const splitTimeline =
    latestUserIndex >= 0 && (busy || latestUserIndex + 1 < renderedTimeline.length);
  const leadingTimeline = splitTimeline
    ? renderedTimeline.slice(0, latestUserIndex + 1)
    : renderedTimeline;
  const liveClusterTimeline = splitTimeline
    ? renderedTimeline.slice(latestUserIndex + 1)
    : [];
  const showLiveCluster = splitTimeline;

  return (
    <section
      className="tc-transcript"
      aria-label="active-session"
      ref={transcriptRef}
    >
      {renderCluster(leadingTimeline, false)}
      {showLiveCluster ? (
        <div className="tc-live-cluster" data-testid="live-cluster">
          {renderCluster(liveClusterTimeline, busy)}
        </div>
      ) : busy ? <ProgressRow busy /> : null}
      <div
        aria-hidden="true"
        className="tc-transcript__spacer"
        data-testid="transcript-spacer"
        style={{ height: `${bottomSpacerHeight}px` }}
      />
    </section>
  );
}
