import { render, screen } from "@testing-library/react";
import { expect,it } from "vitest";
import { InvocationChip } from "./InvocationChip";

it("shows only the invocation label, without icon or remove button",()=>{
  const {container}=render(<InvocationChip instruction={{type:"instruction",kind:"command",resourceId:"command:a",label:"/review",path:".cursor/commands/review.md"}} />);
  const chip=screen.getByLabelText("Command /review");
  expect(chip.textContent).toBe("/review");
  expect(chip.getAttribute("title")).toBe("Command · .cursor/commands/review.md");
  expect(chip.className).toContain("tc-chip--invocation");
  expect(container.querySelector("button,.codicon")).toBeNull();
});
