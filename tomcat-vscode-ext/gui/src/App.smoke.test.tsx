import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import type { VsCodeApiLike } from "./types";

describe("App render smoke test", () => {
  it("mounts the full chat tree and its ModelPicker without throwing", () => {
    const vscodeApi: VsCodeApiLike = {
      getState: vi.fn(() => undefined),
      postMessage: vi.fn(),
      setState: vi.fn(),
    };

    expect(() => render(<App vscodeApi={vscodeApi} />)).not.toThrow();
    expect(screen.getByTestId("composer-input")).toBeTruthy();
    expect(screen.getByTestId("model-select")).toBeTruthy();
  });
});
