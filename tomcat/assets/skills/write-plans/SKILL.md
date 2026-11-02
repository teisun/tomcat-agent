---
name: write-plans
description: Write or revise visually explained, reviewable, executable development plans. Use when designing an implementation, organizing complex changes, or rewriting a technical plan for readers without project background; organize it around the overall solution, individual changes, decisions and trade-offs, tests, and delivery evidence, not direct code implementation.
---

# Development Plan Writing Steps

1. First read the [Tomcat Plan Workflow](references/tomcat-plan-workflow.md).
2. Then read the [Plan Writing Standards](references/plan-structure.md).
3. Start writing the plan.


# Final Check

## Avoid over-design

1. Emphasis: Keep the design elegant while strictly avoiding over-design.
2. Over-design, in functionality or in tests, includes but is not limited to:
   - A design for a scenario outside the requested change or its impact, whose removal would not fail any acceptance case derived from the user's requirements.
   - Locks or versioning for a single-user local workflow; extra layers of abstraction for a single implementation; new storage or protocols when existing mechanisms suffice; state branches with no user-visible effect.
   - Not reusing existing code or infrastructure when reuse is possible. For every proposed new mechanism, search the project for reusable code or infrastructure first. State what exists, what is missing, and why direct reuse or extension cannot satisfy the confirmed requirements, citing concrete files, symbols, or tests. Prefer reuse or a minimal extension when it suffices.
   - Functionality or tests that TDD reasoning shows go beyond the requirements: first derive the acceptance cases from the user's requirements and the change's impact, then keep only the functionality needed to pass them and only the tests needed to verify them.
   - Redundant safety checks: once the user confirms an action, they have accepted the consequences the confirmation describes. Do not add code that re-checks for those consequences before running and blocks the action; if a consequence matters, state it in the confirmation instead. Add a check only for a risk the user has not been told about and has not accepted. For example, if the confirmation says later edits will be overwritten, do not check for later edits before overwriting.
3. Before finishing any plan change (the first draft, each revision, and the content you add after the plan review returns), perform a deletion pass and remove over-designed items and their related content.


## Reader check:
- Before delivery, review the document from the perspective of someone who has not seen the conversation:
  - Can they answer why the change is needed, what will change, why this approach was chosen, and how completion will be verified?
  - Which terms are unexplained?
  - Which necessary background facts are missing or silently assumed?
  - Do diagrams, explanations, decisions, or tasks contradict one another or leave ambiguous instructions?
- Answers must be supported by the document, not supplied from the author's unstated knowledge. Correct the specific gaps found. Use the current writing or review turn for this check; it does not require another model call or review workflow, and self-review must not be reported as independent reader validation.
