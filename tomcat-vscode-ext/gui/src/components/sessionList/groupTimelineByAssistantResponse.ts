import type {
  WebviewMessageBlock,
  WebviewThinkingBlock,
  WebviewTimelineItem,
  WebviewToolCard,
} from "../../types";

export interface AssistantResponseGroup {
  assistantMessageId: string;
  preamble?: WebviewMessageBlock;
  thinking?: WebviewThinkingBlock;
  tools: WebviewToolCard[];
  type: "assistant-response-group";
}

export type GroupedTimelineEntry = AssistantResponseGroup | WebviewTimelineItem;

function belongsToGroup(item: WebviewTimelineItem, groupId: string): boolean {
  if (item.type === "tool") {
    return item.assistantMessageId === groupId;
  }
  if (item.type === "thinking") {
    return item.assistantMessageId === groupId;
  }
  if (item.type === "message" && item.kind === "assistant") {
    return item.assistantMessageId === groupId;
  }
  return false;
}

function collectGroup(
  timeline: WebviewTimelineItem[],
  groupId: string,
): AssistantResponseGroup {
  let preamble: WebviewMessageBlock | undefined;
  let thinking: WebviewThinkingBlock | undefined;
  const tools: WebviewToolCard[] = [];

  for (const item of timeline) {
    if (!belongsToGroup(item, groupId)) {
      continue;
    }
    if (item.type === "message" && item.kind === "assistant") {
      preamble = item;
    } else if (item.type === "thinking") {
      thinking = item;
    } else if (item.type === "tool") {
      tools.push(item);
    }
  }

  return {
    assistantMessageId: groupId,
    preamble,
    thinking,
    tools,
    type: "assistant-response-group",
  };
}

export function groupTimelineByAssistantResponse(
  timeline: WebviewTimelineItem[],
): GroupedTimelineEntry[] {
  const groupIdsWithTools = new Set<string>();
  for (const item of timeline) {
    if (item.type === "tool" && item.assistantMessageId) {
      groupIdsWithTools.add(item.assistantMessageId);
    }
  }

  const emittedGroupIds = new Set<string>();
  const consumedItemIds = new Set<string>();
  const result: GroupedTimelineEntry[] = [];

  for (const item of timeline) {
    if (consumedItemIds.has(item.id)) {
      continue;
    }

    const groupId =
      item.type === "tool"
        ? item.assistantMessageId
        : item.type === "thinking" || (item.type === "message" && item.kind === "assistant")
          ? item.assistantMessageId
          : undefined;

    if (groupId && groupIdsWithTools.has(groupId) && !emittedGroupIds.has(groupId)) {
      const group = collectGroup(timeline, groupId);
      for (const member of timeline) {
        if (belongsToGroup(member, groupId)) {
          consumedItemIds.add(member.id);
        }
      }
      emittedGroupIds.add(groupId);
      result.push(group);
      continue;
    }

    if (
      groupId &&
      groupIdsWithTools.has(groupId) &&
      emittedGroupIds.has(groupId)
    ) {
      continue;
    }

    result.push(item);
  }

  return result;
}
