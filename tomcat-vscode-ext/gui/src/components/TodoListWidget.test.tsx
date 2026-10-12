import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { SessionDock as TodoListWidget } from "./SessionDock";

describe("TodoListWidget", () => {
  it("renders a collapsed current task title and expands into the todo list", () => {
    render(
      <TodoListWidget sessionId="s" onIntent={() => undefined}
        busy
        planState="executing"
        planTodos={[
          { content: "Read the file", id: "1", status: "completed" },
          { content: "Render the transcript UI", id: "2", status: "in_progress" },
          { content: "Verify the screenshot", id: "3", status: "pending" },
        ]}
        sessionTodos={[]}
      />,
    );

    expect(screen.getByTestId("todo-widget-title").textContent).toBe(
      "Render the transcript UI (2/3)",
    );
    expect(screen.getByTestId("todo-widget-title").className).toContain("tc-loading-shimmer");

    fireEvent.click(screen.getByTestId("todo-widget-toggle"));

    expect(screen.getByTestId("todo-widget-title").textContent).toBe("Todos (2/3)");
    expect(screen.getAllByTestId("todo-widget-item")).toHaveLength(3);
    expect(
      document.querySelector(".tc-todo-widget__status--in-progress.codicon-record"),
    ).toBeTruthy();
  });

  it("stays hidden without todo data", () => {
    const { container } = render(
      <TodoListWidget sessionId="s" onIntent={() => undefined}
        busy
        planState={null}
        planTodos={[]}
        sessionTodos={[]}
      />,
    );

    expect(container.innerHTML).toBe("");
  });

  it("stays hidden when the plan is completed", () => {
    const { container } = render(
      <TodoListWidget sessionId="s" onIntent={() => undefined}
        busy
        planState="completed"
        planTodos={[{ content: "All done", id: "1", status: "completed" }]}
        sessionTodos={[]}
      />,
    );

    expect(container.innerHTML).toBe("");
  });

  it("stays hidden when the session is idle", () => {
    const { container } = render(
      <TodoListWidget sessionId="s" onIntent={() => undefined}
        busy={false}
        planState="planning"
        planTodos={[{ content: "Plan", id: "1", status: "pending" }]}
        sessionTodos={[]}
      />,
    );

    expect(container.innerHTML).toBe("");
  });
});
