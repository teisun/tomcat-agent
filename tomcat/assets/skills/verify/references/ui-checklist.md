# UI acceptance checklist

Use this checklist after changing a user-visible webpage, frontend component, or
VS Code webview. Load it from the `verify` skill only when the task has UI scope.

## Before capture

- Identify the documented development-server command and the URL it actually prints.
- Start the server as a background bash task; wait until it is ready.
- Run `node <work_dir>/skills/verify/scripts/bootstrap.mjs` if the managed
  Playwright dependency or Chromium is absent.
- Identify the changed user journey and any required authentication, fixture data,
  or feature flags.

## Capture structural, visual, and runtime evidence

- Run `shot.mjs` as a background task for the desktop viewport.
- For responsive UI, repeat at a narrow viewport (normally `390x844`).
- Read the PNG: blank screen, clipping, overflow, overlap, position, spacing,
  contrast, and the changed interaction state.
- Read the ARIA snapshot: required roles, labels, values, focus, disabled state,
  and dialog/menu expanded state.
- Read the console report: no uncaught error and no `console.error`.
- Curl or fetch critical JavaScript, CSS, image, font, and API resources when
  the rendered result could be incomplete despite an HTML `200`.

## Interaction choice

- Use deterministic `shot.mjs` only when all interaction steps and selectors are
  known before execution.
- Use Playwright MCP when the next action depends on an observed intermediate
  state, or when browser state must survive multiple model-tool turns.
- For an MCP screenshot that the model must see, omit `filename` and
  `fullPage=true`; current `@playwright/mcp` returns only a file link for named
  or full-page screenshots.
- Preserve evidence after each important state transition; do not infer a
  hover/menu/drag result without observing it.

## Evidence handoff

- Record every successful shot command and task ID in the acceptance todo's evidence.
- Keep failed artifacts and report the concrete cause rather than claiming UI
  acceptance passed.
- A passing lint/build/test suite alone is never visual acceptance evidence.
