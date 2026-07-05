import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { selectActiveTodoSource, useActiveTodoProgress } from "./useActiveTodoProgress";

describe("useActiveTodoProgress", () => {
  it("uses planTodos during plan executing", () => {
    const { result } = renderHook(() =>
      useActiveTodoProgress({
        busy: true,
        planState: "executing",
        planTodos: [
          { content: "Build", id: "1", status: "completed" },
          { content: "Test", id: "2", status: "in_progress" },
          { content: "Ship", id: "3", status: "pending" },
        ],
        sessionTodos: [{ content: "Chat todo", id: "s1", status: "in_progress" }],
      }),
    );

    expect(result.current).toMatchObject({
      activeTodo: { content: "Test", id: "2", status: "in_progress" },
      current: 2,
      total: 3,
      title: "Test",
    });
  });

  it("uses sessionTodos in chat when in_progress exists", () => {
    const { result } = renderHook(() =>
      useActiveTodoProgress({
        busy: true,
        planState: "chat",
        planTodos: [],
        sessionTodos: [{ content: "Fix bug", id: "s1", status: "in_progress" }],
      }),
    );

    expect(result.current?.title).toBe("Fix bug");
  });

  it("returns null when no active todo source", () => {
    const { result } = renderHook(() =>
      useActiveTodoProgress({
        busy: true,
        planState: "chat",
        planTodos: [],
        sessionTodos: [{ content: "Later", id: "s1", status: "pending" }],
      }),
    );

    expect(result.current).toBeNull();
  });

  it("selects planTodos as the active source in plan mode", () => {
    expect(
      selectActiveTodoSource({
        busy: true,
        planState: "planning",
        planTodos: [{ content: "Plan", id: "1", status: "pending" }],
        sessionTodos: [{ content: "Chat", id: "2", status: "in_progress" }],
      }),
    ).toEqual([{ content: "Plan", id: "1", status: "pending" }]);
  });

  it("computes current as completed+1 when in_progress exists", () => {
    const { result } = renderHook(() =>
      useActiveTodoProgress({
        busy: true,
        planState: "executing",
        planTodos: [
          { content: "A", id: "1", status: "completed" },
          { content: "B", id: "2", status: "in_progress" },
        ],
        sessionTodos: [],
      }),
    );

    expect(result.current).toMatchObject({ current: 2, total: 2 });
  });
});
