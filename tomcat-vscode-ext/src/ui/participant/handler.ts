import * as vscode from "vscode";

import type { InitializeResult } from "../../serveClient/initialize";
import type { SessionRouter } from "../../serveClient/sessionRouter";
import type { TomcatMessenger } from "../../serveClient/TomcatMessenger";
import { ParticipantTurnRenderer } from "./render";
import type { ParticipantCommands } from "./commands";
import type { VsCodeIde } from "../../ide/VsCodeIde";
import {
  normalizePlanState,
  planStateProgressLabel,
} from "./planState";
import type { SessionOwnershipTracker } from "../webview/ownership";
import type { TomcatUiMode } from "../webview/protocol";

export interface ParticipantHandlerDeps {
  commands: ParticipantCommands;
  getUiMode(): TomcatUiMode;
  ide: VsCodeIde;
  initialize(): Promise<InitializeResult>;
  messenger: TomcatMessenger;
  ownership: SessionOwnershipTracker;
  sessionRouter: SessionRouter;
}

function buildErrorResult(message: string, sessionId: string): vscode.ChatResult {
  return {
    errorDetails: {
      message,
    },
    metadata: {
      sessionId,
    },
  };
}

export function createParticipantHandler(
  deps: ParticipantHandlerDeps,
): vscode.ChatRequestHandler {
  return async (request, context, stream, token) => {
    const initializeResult = await deps.initialize();

    const sessionId = await deps.sessionRouter.resolveSessionId(context.history);
    if (deps.getUiMode() === "webview") {
      return buildErrorResult(
        "Tomcat chat participant is disabled by `tomcat.ui=webview`.",
        sessionId,
      );
    }
    const ownership = deps.ownership.claim(sessionId, "participant");
    if (!ownership.ok && ownership.record.owner === "webview") {
      return buildErrorResult(
        "This Tomcat session is currently owned by the Tomcat webview.",
        sessionId,
      );
    }
    if (request.command && request.command !== "plan" && request.command !== "model") {
      return buildErrorResult(
        `Unknown Tomcat slash command: /${request.command}`,
        sessionId,
      );
    }

    const attachTurn = deps.commands.attachTurn(sessionId, stream);
    const renderer = new ParticipantTurnRenderer(deps.ide, stream);
    let renderQueue = Promise.resolve();
    let markTurnComplete: () => void = () => undefined;
    const turnCompleted = new Promise<void>((resolve) => {
      markTurnComplete = resolve;
    });

    const eventSubscription = deps.messenger.onEvent((event) => {
      if (event.sessionId !== sessionId) {
        return;
      }

      renderQueue = renderQueue
        .then(() => renderer.render(event))
        .catch((error: unknown) => {
          stream.markdown(`\n\nTomcat render error: ${String(error)}`);
        });

      if (event.type === "agent_end") {
        markTurnComplete();
      }
    });

    const cancellation = token.onCancellationRequested(() => {
      void deps.messenger.request({
        sessionId,
        type: "interrupt",
      }).catch(() => undefined);
    });

    try {
      if (request.command === "plan") {
        const outcome = await deps.commands.handlePlanSlashCommand({
          initializeResult,
          messenger: deps.messenger,
          request,
          sessionId,
          sessionRouter: deps.sessionRouter,
          stream,
        });
        if (outcome.error) {
          return buildErrorResult(outcome.error, sessionId);
        }
        if (outcome.awaitAgentEnd) {
          await turnCompleted;
          await renderQueue;
        }
        return {
          metadata: deps.sessionRouter.buildResultMetadata(sessionId),
        };
      }

      if (request.command === "model") {
        const outcome = await deps.commands.handleModelSlashCommand({
          initializeResult,
          messenger: deps.messenger,
          request,
          sessionId,
          sessionRouter: deps.sessionRouter,
          stream,
        });
        if (outcome.error) {
          return buildErrorResult(outcome.error, sessionId);
        }
        return {
          metadata: deps.sessionRouter.buildResultMetadata(sessionId),
        };
      }

      const state = await deps.sessionRouter.getState(sessionId).catch(() => null);
      const planState = normalizePlanState(state?.planState);
      if (planState && planState !== "chat") {
        stream.progress(planStateProgressLabel(planState, state?.planId));
      }

      const commandType = context.history.length === 0 ? "prompt" : "follow_up";
      const response = await deps.messenger.request({
        params: {
          attachments: [],
        },
        sessionId,
        text: request.prompt,
        type: commandType,
      });

      if (!response.success) {
        return buildErrorResult(
          response.error ?? "Tomcat request failed",
          sessionId,
        );
      }

      await turnCompleted;
      await renderQueue;

      return {
        metadata: deps.sessionRouter.buildResultMetadata(sessionId),
      };
    } finally {
      deps.ownership.release(sessionId, "participant");
      cancellation.dispose();
      eventSubscription.dispose();
      attachTurn.dispose();
    }
  };
}
