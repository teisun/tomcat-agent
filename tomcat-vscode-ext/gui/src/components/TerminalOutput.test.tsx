import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { tailTerminalOutput, TerminalOutput } from "./TerminalOutput";

describe("TerminalOutput", () => {
  it("keeps only the last n lines for previews", () => {
    expect(tailTerminalOutput("one\ntwo\nthree\nfour", 2)).toBe("three\nfour");
  });

  it("renders command output in monospace preformatted text", () => {
    render(<TerminalOutput text={"line one\nline two"} />);

    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("line one");
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("line two");
  });

  it("prepends a `$ <command>` prompt line when a command is given", () => {
    render(<TerminalOutput command="git status" text={"On branch main"} />);

    expect(screen.getByTestId("terminal-output-cmd").textContent).toContain("$ git status");
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("$ git status");
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain("On branch main");
  });

  it("omits the prompt line when no command is provided", () => {
    render(<TerminalOutput text={"just output"} />);

    expect(screen.queryByTestId("terminal-output-cmd")).toBeNull();
  });
});
