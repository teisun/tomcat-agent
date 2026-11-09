# Plan Writing Standards

Technical design, prose, and visual guidelines for development plans.

## Technical Design and explanation standards

- Reason from first principles: when planning or coding, work out the architecture and implementation from first principles, follow best practices, pursue the most elegant solution, and dare to overturn a flawed technical design rather than patch around it.
- Keep the design elegant while strictly avoiding over-design.
- Put user experience first: when a task involves UI, design it from the user's experience and above all follow the existing UI design conventions of the user's project.

## Writing Requirements

### Prose Rules

- Explain problems and technical solutions in a way that is easy to read and understand. The following requirements apply to the plan document, not to chat replies or review reports.

- Assume the reader has no project background. Explain the context needed for each chapter, define unfamiliar terms on first use, and use one consistent name for each concept.

- Give each paragraph one idea and put its main point first. Describe concrete behavior and outcomes. Use lists for steps and distinct items, and short paragraphs for reasoning.

- Remove empty assurances, promotional adjectives, and repetition. Keep necessary context, evidence, risks, and trade-offs in the plan itself, not only in chat.

### Wording Examples

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

## Visual and Verbal Expression

### Visual and Verbal Principles

- Combine visuals and text, leading with the visual (ASCII diagram) and keeping text in a supporting role; a picture is worth a thousand words, and all written output must be clear enough for a complete beginner to understand.

### Diagram Rules

- Add local diagrams for complex logic: trace relevant success/failure branches. For UI, show layout, controls, and feedback, not a list of text labels.
- Give each diagram one main takeaway and state it in the caption. Draw the relevant actions and paths, not just boxes of component names: label arrows with verbs such as "read draft", "save settings", and "notify UI". Make added, removed, or changed paths visible; keep names and directions consistent in comparisons. An overall takeaway may connect several components or changes.

### Diagram Example (Example 1)

```text
Form (keeps input) --submit--> Save entry --write--> Storage
                                |-- success: show saved
                                +-- failure: show reason and retry

+-- Edit settings ----------------+
| Name [user input              ] |
| Save failed; input kept         |
|                        [Retry]  |
+--------------------------------+
```

Takeaway: a failed save keeps input. Follow with verified implementation/test paths and the four-field case in Plan Structure.
