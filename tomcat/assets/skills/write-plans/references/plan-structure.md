# Plan Writing Standards

This document organizes plan-writing requirements by topic and provides a chapter skeleton and application examples.

## Writing Requirements

- Explain problems and technical solutions in a way that is easy to read and understand. The following requirements apply to the plan document, not to chat replies or review reports.

- Assume the reader has no project background. Explain the context needed for each chapter, define unfamiliar terms on first use, and use one consistent name for each concept.

- Put the complete explanation in the plan itself, not only in the chat.

## Technical Design and explanation standards

- Reason from first principles: when planning or coding, work out the architecture and implementation from first principles, follow best practices, pursue the most elegant solution, and dare to overturn a flawed technical design rather than patch around it.
- Keep the design elegant while strictly avoiding over-design.
- Put user experience first: when a task involves UI, design it from the user's experience and above all follow the existing UI design conventions of the user's project.



## 1. Chapter Skeleton (Recommended Order)


```text
Goal: why do this, and what will be delivered?
  |
Plan: overall diagram + one sentence
  -> glossary (optional) -> solution overview (not needed for small plans)
  -> separate subsections for each problem/requirement: background, solution, key decisions, and tests
  -> add implementation locations and implementation/verification arrangements as needed
  -> acceptance criteria
  |
Todos Board: tasks and status
```

- `MUST`: explain whenever applicable; simple content may be combined.
- `SHOULD`: recommended; do not repeat what another chapter already explains.
- `OPTION`: add a separate chapter as needed.

Do not generate empty tables for inapplicable content. Omitting an OPTION chapter does not mean omitting necessary information: put execution prerequisites, task dependencies, and acceptance methods in the relevant solution, task, or acceptance requirements.

| Location / Chapter | Requirement Level | When to Write It and What It Answers |
|---|---|---|
| `## Goal` | MUST | Objective, observable deliverables, and scope boundaries. |
| Start of `## Plan`: overall ASCII diagram + one sentence | MUST | Show the trigger, key processing, outcome, and important branches; summarize the change and its significance in one sentence. |
| `### Glossary` | OPTION | Collect easily confused roles, abbreviations, or states; explain a small number of terms inline instead of creating a separate chapter. |
| `### Solution Overview` | SHOULD | Explain the overall flow, responsibilities, and boundaries first, then the key design; small plans do not need this chapter. |
| `### Solution Breakdown and Implementation` | MUST | Use separate subsections to explain the background, solution, key decisions, and tests for each problem or requirement. |
| `### Implementation Locations` | SHOULD | Summarize major files and changes when several locations are involved; explain a single-location change nearby. |
| `### Deletion Review and Triggers for Reconsideration` | OPTION | When there are many trade-offs, collect what was removed, why items remain, and when to reconsider; otherwise include this in key decisions. |
| `### Implementation and Verification Arrangements` | OPTION | When coordination is needed, collect environment conditions, existing changes or execution restrictions, implementation order and stage-completion evidence, and verification-batch timing, commands, and coverage; for small plans, incorporate these into the task list and acceptance. |
| `### Acceptance` | MUST | For a coding plan, state observable overall completion conditions, behavior to review, regression boundaries, expected impact radius, necessary cross-feature checks, checking methods, and delivery close-out; keep specific test cases in their problem or feature chapters. |
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

When an overview is needed, explain the overall flow, responsibilities, and boundaries first, then the key design, so readers can answer:

- **Goal and scope**: What will this deliver, exclude, and preserve unchanged?
- **Responsibilities and reuse**: Who owns each part, what can existing capabilities handle, and where are new capabilities missing?
- **Key flows and boundaries**: How does the user's action lead to a result, and what remains after failure or cancellation? Where permissions, data, compatibility, or persistence are involved, how far do the actual guarantees extend?
- **Completion and delivery**: How does local completion differ from overall delivery, and what inspectable results and deliverables are required?

Keep only the conclusions and key diagrams that determine readers' understanding in the overview. Leave detailed evidence, protocols, state transitions, error handling, and tests in the relevant breakdown, cross-referencing instead of repeating the full text.

### 2.2 Local Skeleton for Each Problem or Requirement

Name chapters after user outcomes, not technical layers or file lists. For internal refactoring, name the behavior or guarantee being preserved.

`MUST`: Format each problem or requirement section heading as `Issue N: Description` or `Requirement N: Description`. Replace `N` with a sequence number and `Description` with the specific problem or intended user-visible outcome. Use the equivalent labels in the plan's language; always include both the label and the number.

Organize each problem or requirement in the order and hierarchy below:

```text
Issue N: Description                       Requirement N: Description
├─ Problem symptoms                        ├─ Requirement background
├─ Root cause and evidence                 ├─ Solution
├─ Solution                                │  ├─ Solution explanation (diagram + text)
│  ├─ Solution explanation (diagram + text)  │  ├─ ASCII UI mockup (if UI is involved)
│  ├─ ASCII UI mockup (if UI is involved)    │  └─ Key decisions checklist
│  └─ Key decisions checklist               └─ Test case checklist
└─ Test case checklist
```

- Do not invent a root cause for a new requirement.
- Within the solution explanation, move from the expected experience to the mechanism and then the implementation locations. This is an explanation sequence, not an additional set of subsection headings.
- Explain the reasons and trade-offs behind the choices in the solution explanation.
- The glossary, overview, implementation locations, and implementation/verification arrangements are not problem/requirement chapters; do not repeat the local skeleton for them.

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

For the relationship between requirements and tests, see item 2 of [Avoid over-design](../SKILL.md#avoid-over-design).

#### Command Evidence

- Source commands from the user, project rules, manifests,
  documentation, or nearby existing tests; never invent defaults.

#### Case Format

```text
T-ID | Case name (mark P0/P1 when there is a real risk classification)
- Scenario: What prerequisites and risk make this case necessary?
- Steps: How do existing test entrypoints trigger the actual behavior or failure?
- Expected result: Which states, files, responses, or side effects are observed; what must and must not happen?
- Files involved: Identify test files to add / modify / reuse; mark unimplemented ones as proposed additions.
```

### 2.5 Visual and Verbal Expression

#### Visual and Verbal Principles

- Combine visuals and text, leading with the visual (ASCII diagram) and keeping text in a supporting role; a picture is worth a thousand words, and all written output must be clear enough for a complete beginner to understand.

#### Diagram Rules

- add local diagrams for complex local logic.
- Give each diagram one main takeaway and state it in the caption. Draw the relevant actions and paths, not just boxes of component names: label arrows with verbs such as "read draft", "save settings", and "notify UI". Make added, removed, or changed paths visible; keep names and directions consistent in comparisons. An overall takeaway may connect several components or changes.

#### Prose Rules

- Give each paragraph one idea and put its main point first. Describe concrete behavior and outcomes. Remove empty assurances, promotional adjectives, and sentences that add no information; avoid excessive bolding, lists, or emoji. Use lists for steps and distinct items, and short paragraphs for reasoning. Brevity must not remove necessary evidence, risks, or trade-offs.

#### Wording Examples

```text
    Examples of clearer wording:
        Example 1: Replace vague claims with concrete behavior.
            Vague: Optimize state management to comprehensively improve reliability and consistency.
            Concrete: If saving fails, keep the user's input and show a retry button so they do not have to fill in the form again.

        Example 2: Connect technical changes to what the user can do.
            Hard to follow: Storage-layer changes → service-layer changes → UI changes.
                A beginner cannot tell what these changes mean for them.

            Easier to follow: Recover an accidentally deleted project.
                ├─ What the user sees: the project moves to Trash.
                ├─ How it works: mark the project as deleted without removing its data yet.
                └─ Implementation locations: the relevant interfaces, files, and tests.
```

## 3. Connect the Solution to Implementation and Acceptance

### 3.1 Tasks and Implementation Order

For task decomposition, see item 6 of the Behavior contract in the [Tomcat Plan Workflow](tomcat-plan-workflow.md).

State modification locations, necessary dependencies, and completion conditions in the task list. To explain sequencing, use a dependency diagram or short paragraph describing stage handoff outputs, referencing task IDs without creating separate checkable progress.

### 3.2 Acceptance Criteria

For acceptance content, task constraints, and documentation close-out, see [Chapter 1](#1-chapter-skeleton-recommended-order).

### 3.3 Implementation and Verification Arrangements (As Needed)

When coordination is needed, explain implementation notes, implementation order and stage-completion evidence, and verification batches in this one chapter; they do not require three standalone chapters. Milestones are delivery boundaries, and verification batches are shared build/test boundaries; for small plans, incorporate this information into the task list and acceptance.

For test design principles and command evidence, see [2.4 Test Cases](#24-test-cases-prove-behavior-not-that-names-exist). The following is an optional verification-batch format:

| Batch | Timing / Working Directory / Command and Evidence | Cases Covered | What It Independently Proves / Reuse Conditions |
|---|---|---|---|
| A batch with an actual need | Cite existing project entrypoints and file evidence; explicitly mark proposed targets | Reference case IDs in their chapters | Explain the risk boundary; reuse results whose code, commands, and environment remain valid |

## 4. Simplifying Small Changes

A single-location defect can be written as:

```text
Goal: behavior to fix, and what explicitly remains unchanged
Plan: one concise flow diagram + one sentence
  One problem chapter: symptoms -> root-cause evidence -> visual solution and decisions -> focused tests
  Acceptance criteria: actual behavior, necessary regressions, evidence-based check commands, post-acceptance documentation
Todos Board: implementation and tests -> final acceptance
```
