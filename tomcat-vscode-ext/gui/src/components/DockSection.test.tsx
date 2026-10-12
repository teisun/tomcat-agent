import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { DockSection } from "./DockSection";
afterEach(cleanup);
it("independent headers have unique controls and sibling actions do not toggle", () => {
  const action = vi.fn(); render(<><DockSection label="files" title="Files" actions={<button onClick={action}>Undo All</button>}><p>Files content</p></DockSection><DockSection label="todos" title="Todos"><p>Todo content</p></DockSection></>);
  const files = screen.getByRole("button", { name: "Expand files" }), todos = screen.getByRole("button", { name: "Expand todos" });
  expect(files.getAttribute("aria-controls")).not.toBe(todos.getAttribute("aria-controls"));
  fireEvent.click(screen.getByRole("button", { name: "Undo All" })); expect(action).toHaveBeenCalledTimes(1); expect(files.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(files); fireEvent.click(todos); expect(files.getAttribute("aria-expanded")).toBe("true"); expect(todos.getAttribute("aria-expanded")).toBe("true");
});
