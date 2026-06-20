import * as vscode from "vscode";

import type { VsCodeIde } from "../../ide/VsCodeIde";
import type { WireEvent } from "../../serveClient/wire";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isMutationTool(toolName: string): boolean {
  return toolName === "write" || toolName === "edit" || toolName === "hashline_edit";
}

function asText(value: unknown): string | undefined {
  if (typeof value === "string") {
    return value;
  }

  if (value === null || value === undefined) {
    return undefined;
  }

  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function getAssistantDelta(event: WireEvent): { delta: string; kind: string } | undefined {
  if (event.type !== "message_update" || !isRecord(event.assistantMessageEvent)) {
    return undefined;
  }

  const delta = event.assistantMessageEvent.delta;
  const kind = event.assistantMessageEvent.kind;
  if (typeof delta !== "string" || typeof kind !== "string") {
    return undefined;
  }

  return { delta, kind };
}

export class ParticipantTurnRenderer {
  private hasShownThinkingNotice = false;

  constructor(
    private readonly ide: VsCodeIde,
    private readonly stream: vscode.ChatResponseStream,
  ) {}

  async render(event: WireEvent): Promise<void> {
    switch (event.type) {
      case "agent_start":
        this.stream.progress("Tomcat agent started");
        return;
      case "message_update": {
        const delta = getAssistantDelta(event);
        if (!delta) {
          return;
        }

        if (delta.kind === "content_delta") {
          this.stream.markdown(delta.delta);
          return;
        }

        if (delta.kind === "thinking_delta" && !this.hasShownThinkingNotice) {
          this.hasShownThinkingNotice = true;
          this.stream.progress("Tomcat is thinking...");
        }
        return;
      }
      case "tool_execution_start":
        this.stream.progress(`Running ${event.toolName}`);
        if (isMutationTool(event.toolName)) {
          await this.ide.rememberToolStart(event.toolCallId, event.args);
        }
        return;
      case "tool_call_streaming":
      case "tool_execution_update":
        this.stream.progress(`Updating ${event.toolName}`);
        return;
      case "tool_execution_end":
        await this.renderToolEnd(event);
        return;
      case "llm_notice":
        this.stream.progress(event.message);
        return;
      case "llm_error":
        this.stream.markdown(
          `\n\nTomcat error: \`${event.reason}\` - ${event.errorMessage}`,
        );
        return;
      case "agent_interrupted":
        this.stream.progress("Tomcat turn interrupted");
        return;
      case "agent_end":
        if (event.error) {
          this.stream.markdown(`\n\nTomcat finished with error: ${event.error}`);
        }
        return;
      default:
        return;
    }
  }

  private async renderToolEnd(
    event: Extract<WireEvent, { type: "tool_execution_end" }>,
  ): Promise<void> {
    const summary = asText(event.result);
    const outcome = event.isError ? "failed" : "finished";

    this.stream.progress(`${event.toolName} ${outcome}`);
    if (summary) {
      this.stream.markdown(`\n\n**${event.toolName}**\n\n\`\`\`text\n${summary}\n\`\`\``);
    }

    if (!event.display) {
      return;
    }

    if (event.display.kind === "file") {
      const change = await this.ide.rememberToolResult(
        event.toolCallId,
        event.display.file,
      );
      this.stream.anchor(
        this.ide.createFileAnchor(change.displayPath),
        change.displayPath,
      );
      this.stream.button({
        arguments: [{ toolCallId: event.toolCallId }],
        command: "tomcat.openDiff",
        title: "Open Diff",
      });
      this.stream.button({
        arguments: [{ toolCallId: event.toolCallId }],
        command: "tomcat.applyEdit",
        title: "Apply Edit",
      });
      return;
    }

    if (event.display.kind === "plan") {
      this.stream.markdown(`\n\n\`\`\`md\n${event.display.plan}\n\`\`\``);
      return;
    }

    if (event.display.kind === "text") {
      this.stream.markdown(`\n\n${event.display.text}`);
    }
  }
}
