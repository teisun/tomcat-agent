# Plan Structure

This document defines plan sections and structural examples. Read [Plan Writing Standards](writing-standards.md) for design and writing guidance.

## 1. Chapter Skeleton (Recommended Order)


```text
Goal: why do this, and what will be delivered?
  |
Plan: overall diagram + one-sentence overview
  -> solution overview (inside Plan; omit for small plans)
  |
Solution Breakdown and Implementation
  -> each problem/requirement:
     -> symptoms / requirement; root cause and evidence (issues only)
     -> solution explanation (ASCII + prose); UI mockup (as needed)
     -> key decisions table -> test cases
  |
Other optional chapters -> Acceptance -> Todos Board
```

- `MUST`: include every applicable required section; keep sections separate even when their content is brief.
- `SHOULD`: recommended; do not repeat what another chapter already explains.
- `OPTION`: add a separate chapter as needed.

Use the fixed chapter titles below, or their equivalents in the plan's language.

| Chapter Title (Follow These Names Exactly) | Requirement Level | When to Write It and What It Answers |
|---|---|---|
| `## Goal` | MUST | Objective, observable deliverables, and scope boundaries. |
| Start of `## Plan`: overall ASCII diagram + one sentence | MUST | Show the trigger, key processing, outcome, and important branches; summarize the change and its significance in one sentence. |
| `### Solution Overview` (inside Plan) | SHOULD | Explain the overall flow and core design. Small plans can omit it. |
| `## Solution Breakdown and Implementation` | MUST | Give each problem or requirement its own section, with the following headings and nesting. |
| `### Issue N: Description` / `### Requirement N: Description` | MUST | Label and number each issue or requirement, followed by a one-sentence description. |
| `#### Problem Symptoms` / `#### Requirement Description` | MUST | Describe the observed problem or the requested outcome. |
| `#### Root Cause and Evidence` | MUST (issues only) | Explain the cause with evidence; use prose and add an ASCII diagram as needed. Omit this section for requirements. |
| `#### Solution` | MUST | Contain the solution explanation and, when needed, a UI mockup as separate subsections. |
| `##### Solution Explanation` | MUST | Explain the solution with an ASCII diagram plus prose. |
| `##### UI Mockup` | OPTION | Show the proposed UI with an ASCII diagram as needed; otherwise omit this subsection. |
| `#### Key Decisions Table` | MUST | Follow Solution at the same heading level. Include the six-column table from §2.3, not just prose or a reference to the table. |
| `#### Test Cases` | MUST | Follow Key Decisions Table at the same heading level. Describe scenarios, steps, expected results, and test file paths using §2.4. |
| Other optional chapters | OPTION | Add supporting content as needed; use `##` for standalone chapters. |
| `## Acceptance` | MUST | For a coding plan, state observable overall completion conditions, behavior to review, regression boundaries, expected impact radius, necessary cross-feature checks, checking methods, and delivery close-out; keep specific test cases in their problem or feature chapters. |
| `## Todos Board` | MUST | Use the host's task board for implementation tasks, dependencies, and status; refer to chapters for technical details and to task IDs in prose; do not create a second task list or progress record. |

### Acceptance Todo Constraints

- For a coding plan, always add one final todo with `kind=acceptance`. Its
  content is ordinary human language, for example "Acceptance: review the
  model-deletion flow and regress the affected cases". It must be the final
  acceptance todo; milestone or intermediate verification todos use `kind=work`.
- A plan may contain at most one `kind=acceptance`; `create_plan` and
  `update_plan` reject a second one.

### Documentation Close-out

- After acceptance is complete, update the relevant repository documentation
  to capture the technical designs introduced or changed in this iteration,
  reflecting the implementation that was actually delivered.

## 2. Explain the Whole First, Then Each Part

### 2.1 Solution Overview

Keep `### Solution Overview` inside `## Plan`; explain the overall flow and core design. Small plans can omit it. Define terms on first use, not in a separate glossary.

### 2.2 Local Skeleton for Each Problem or Requirement

Name chapters after user outcomes, not technical layers or file lists. For internal refactoring, name the behavior or guarantee being preserved.

`MUST`: Format each problem or requirement section heading as `Issue N: Description` or `Requirement N: Description`. Replace `N` with a sequence number and `Description` with the specific problem or intended user-visible outcome. Use the equivalent labels in the plan's language; always include both the label and the number.

Use separate Markdown headings in the order and hierarchy below, following the levels in the chapter table. Bold text and list items are not substitutes for headings. Keep every required subsection, even in a small plan.

```text
Issue N: Description                  Requirement N: Description
├─ Problem Symptoms                   ├─ Requirement Description
├─ Root Cause and Evidence            ├─ Solution
├─ Solution                           │  ├─ Solution Explanation
│  ├─ Solution Explanation            │  └─ UI Mockup (as needed)
│  └─ UI Mockup (as needed)            ├─ Key Decisions Table
├─ Key Decisions Table                └─ Test Cases
└─ Test Cases
```

- Do not invent a root cause for a new requirement.
- Omit Root Cause and Evidence for requirements. For issues, include prose and add an ASCII diagram as needed.
- Solution Explanation includes an ASCII diagram plus prose. UI Mockup is a separate subsection under Solution, included only as needed.
- Within the solution explanation, move from the expected experience to the mechanism and then the implementation locations. This is an explanation sequence, not an additional set of subsection headings.
- Explain the reasons and trade-offs behind the choices in the key decisions table.

### 2.3 Key Decisions: Read the Question, Choice, and Cost Together

The following six-column table is the only format for key decisions:

| Dimension | Concern | Decision | Rationale and Trade-offs | Plain-language Explanation | Notes |
|---|---|---|---|---|---|
| One aspect requiring a decision | What conflict needs resolving now? | State the final choice in one sentence. | Why this choice, can existing work be reused directly, why not a simpler alternative, and what cost is accepted? | Explain the change so someone without project background can understand it. | Use small gray text for specific files/symbols/contracts, before-and-after changes and boundaries; include reconsideration conditions when needed. |

Gray-text markup example: `<small style="color:gray">…</small>`.

### 2.4 Test Cases: Prove Behavior, Not That Names Exist

#### Design Principles

Design verification in proportion to the change's risk and affected surface, not
to the number of todos.

- Design unit, integration, and E2E coverage only where each layer catches a
  distinct failure type; do not add redundant checks merely to make a plan look
  comprehensive.

For the relationship between requirements and tests, see item 2 of [Avoid over-design](final-check.md#avoid-over-design).

#### Command Evidence

- Source commands from the user, project rules, manifests,
  documentation, or nearby existing tests; never invent defaults.

#### Case Format

```text
T-ID | Failed save keeps input (mark P0/P1 only when justified)
- Scenario: Edited form; test storage fails before writing.
- Steps: Save, inspect input and feedback; restore storage and retry.
- Expected result: Failure keeps input and shows no success; retry shows saved.
- Files involved: <verified save-test path>; mark new test files as proposed additions.
```

## 3. Simplifying Small Changes

Use this heading-only skeleton for a small issue, filling each section according to the chapter table. Keep required headings separate even when the content is brief. This example omits an unnecessary UI Mockup; for a requirement, use Requirement N: Description and Requirement Description, and omit Root Cause and Evidence.

```markdown
## Goal
## Plan
## Solution Breakdown and Implementation
### Issue N: Description
#### Problem Symptoms
#### Root Cause and Evidence
#### Solution
##### Solution Explanation
#### Key Decisions Table
#### Test Cases
## Acceptance
## Todos Board
```
