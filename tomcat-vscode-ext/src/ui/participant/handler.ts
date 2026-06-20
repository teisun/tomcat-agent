import * as vscode from "vscode";

import type { InitializeResult } from "../../serveClient/initialize";
import type { SessionRouter } from "../../serveClient/sessionRouter";
import type { TomcatMessenger } from "../../serveClient/TomcatMessenger";
import { ParticipantTurnRenderer } from "./render";
import type { ParticipantCommands } from "./commands";
import type { VsCodeIde } from "../../ide/VsCodeIde";

export interface ParticipantHandlerDeps {
  commands: ParticipantCommands;
  ide: VsCodeIde;
  initialize(): Promise<InitializeResult>;
  messenger: TomcatMessenger;
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
    await deps.initialize();

    const sessionId = await deps.sessionRouter.resolveSessionId(context.history);
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
      cancellation.dispose();
      eventSubscription.dispose();
      attachTurn.dispose();
    }
  };
}
