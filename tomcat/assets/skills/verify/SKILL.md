---
name: verify
description: Discover and run this project's full build/test/lint verification commands (P0–P5 discovery), then report real green-build evidence.
allowed-tools:
  - read
  - search_files
  - list_dir
  - bash
  - task_output
  - task_list
  - task_stop
  - tool_search
  - tool_describe
  - tool_call
  - update_plan
---

## Green-build verification

Produce real evidence that the current code builds and passes its checks — do not
describe what would be tested.

1. First identify the project and its documented acceptance commands. Use this order:
   - P0: an explicit command from the user or the approved plan;
   - P1: repository instructions such as `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md`, or the nearest README;
   - P2: the nearest project manifest and its scripts/tasks;
   - P3: workspace-level configuration or CI workflow;
   - P4: a narrowly scoped smoke command inferred from the changed code;
   - P5: if no runnable command can be found, explain that fact with the files inspected and run the smallest safe parse/type/build check available.

2. Run the project's complete check set: format, lint, full tests, build, and
   project-specific UI checks where they exist. Every acceptance command explicitly
   named in the approved plan is mandatory; you may run additional relevant checks,
   but never omit or replace a named command with a narrower one. Do not invent
   project tests or claim visual checks that do not exist.

Before choosing an output directory, read `workspace.project_resource_dir` from the active Tomcat configuration. It defaults to `.agents`. Pass the resolved project directory explicitly to `--out` so a custom setting such as `.workspace-data` stores screenshots under `.workspace-data/shots`; never rely on the script default for a custom configuration.

3. Start every acceptance command with `bash(run_in_background=true)`. If the next
   step does not strictly depend on its result, do other independent work and wait
   for `<background-task-finished>`. If it does, call `task_output(block=true)` with
   a realistic wait slice until it finishes.

4. A command is valid evidence only when its background task is `Finished` with
   `exit_code=0`, and it started after the newest code edit. Failed, stopped, reused,
   or still-running tasks are not evidence.

5. When all selected checks pass, call `update_plan` with:

```json
{
  "green_build_pass": true,
  "green_build_evidence": [{
    "command": "<the exact background bash command>",
    "task_id": "<finished-background-task-id>"
  }],
  "ops": []
}
```

The runtime validates the task ID, actual command, exit code, and freshness itself.
Never set `green_build_pass` to true without this evidence. If discovery found no
meaningful command, leave the plan open and report the concrete missing acceptance
path rather than fabricating success.

## UI acceptance

For a user-visible web, frontend, or VS Code webview change, verify the rendered
result in addition to the project's normal checks. A build, typecheck, or DOM-only
test does not prove that the page is visible, positioned correctly, or free of
browser runtime errors.

1. Discover the project's documented way to start a local development server. Start
   it with `bash(run_in_background=true)`, then use `task_output` to obtain the
   actual URL and confirm the server is ready before opening it.
2. Prepare the managed browser runtime explicitly when needed:

   ```text
   node <work_dir>/skills/verify/scripts/bootstrap.mjs
   ```

   `bootstrap.mjs` installs the locked Node dependency and Chromium. `shot.mjs`
   never installs dependencies or browsers during acceptance; if they are missing,
   run bootstrap and record its outcome instead of claiming the browser is
   unavailable.
3. Capture the three evidence types with a background task:

   ```text
   node <work_dir>/skills/verify/scripts/shot.mjs <url> \
     --out <workspace>/<project_resource_dir>/shots --name <screen> --viewport 1440x900
   ```

   This writes `<screen>.png` (visual truth), `<screen>.aria.txt` (structure), and
   `<screen>.console.json` (browser runtime). A page error or `console.error` makes
   the shot task fail. Its exact command and successful task ID are valid
   `green_build_evidence`.
4. Read all three artifacts. Use the PNG for blank screens, clipping, overlap,
   spacing, color, and layout; use ARIA for roles, labels, and expanded/disabled
   state; use console output for runtime faults. For responsive UI, capture at
   least desktop and a narrow viewport such as `390x844`.
5. Fetch or curl important JavaScript, CSS, image, or API subresources when
   relevant. HTML `200` alone does not prove that the page's dependent assets or
   data loaded.
6. Do not rationalize a missing check. “Looks right”, “cannot run a browser”, or
   “the screenshot is probably fine” is not evidence. Diagnose the concrete cause:
   dev server URL, browser bootstrap, selector/readiness condition, asset request,
   or browser console error. If it cannot be resolved, leave acceptance open with
   the artifacts and failure evidence.

For a known fixed interaction sequence, encode it in a deterministic headless
script. When the next interaction depends on what the previous page state looks
like, discover configured Playwright tools through the deferred connector path:
`tool_search(source="playwright")` → `tool_describe(names=[...])` →
`tool_call(name="mcp__playwright__...", arguments={...})`. Retain screenshots
and structural evidence from that interaction loop. Never assume Playwright is
configured: if the source search is empty or reports an error, report that fact.

For a visual screenshot that must return to the model through Playwright MCP,
call `tool_call(name="mcp__playwright__browser_take_screenshot", arguments={...})`
**without** `filename` and without `fullPage=true`. Current `@playwright/mcp`
intentionally saves a filename/full-page capture to disk and returns only a
Markdown file link; it omits the image content needed for the model vision loop.
Use the default viewport screenshot for visual judgement.
