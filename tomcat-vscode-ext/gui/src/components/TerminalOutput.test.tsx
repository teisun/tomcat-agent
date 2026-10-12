import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  limitTerminalOutput,
  tailTerminalOutput,
  TerminalOutput,
} from "./TerminalOutput";

describe("TerminalOutput", () => {
  it("keeps only the last n lines for previews", () => {
    expect(tailTerminalOutput("one\ntwo\nthree\nfour", 2)).toBe("three\nfour");
  });

  it("preserves the source trailing newline in a five-line preview", () => {
    expect(tailTerminalOutput("one\ntwo\nthree\nfour\nfive\nsix\n", 5)).toBe(
      "two\nthree\nfour\nfive\nsix\n",
    );
    expect(tailTerminalOutput("one\ntwo\nthree\nfour\nfive\nsix", 5)).toBe(
      "two\nthree\nfour\nfive\nsix",
    );
  });

  it("independently bounds more than 500 short lines", () => {
    const output = Array.from(
      { length: 501 },
      (_, index) => `line-${index}`,
    ).join("\n");
    const limited = limitTerminalOutput(output);
    expect(limited.split("\n")).toHaveLength(500);
    expect(limited).not.toContain("line-0\n");
    expect(limited).toContain("line-500");
  });

  it("bounds one overlong Unicode line without splitting a surrogate pair", () => {
    const output = `${"x"}😀${"y".repeat(29_999)}`;
    const limited = limitTerminalOutput(output);
    expect(output.length).toBe(30_002);
    expect(limited.length).toBe(29_999);
    expect(limited.startsWith("y")).toBe(true);
    expect(limited).not.toContain("�");
  });

  it("renders command output in monospace preformatted text", () => {
    render(<TerminalOutput text={"line one\nline two"} />);

    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "line one",
    );
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "line two",
    );
  });

  it("prepends a `$ <command>` prompt line when a command is given", () => {
    render(<TerminalOutput command="git status" text={"On branch main"} />);

    expect(screen.getByTestId("terminal-output-cmd").textContent).toContain(
      "$ git status",
    );
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "$ git status",
    );
    expect(screen.getByTestId("tool-row-terminal").textContent).toContain(
      "On branch main",
    );
  });

  it("omits the prompt line when no command is provided", () => {
    render(<TerminalOutput text={"just output"} />);

    expect(screen.queryByTestId("terminal-output-cmd")).toBeNull();
  });
});
