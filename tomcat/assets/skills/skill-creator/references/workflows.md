# Workflow Patterns

## Sequential work

Give the agent the whole path before the first action:

1. Inspect the inputs.
2. Produce a small draft.
3. Validate the draft.
4. Make the final output.
5. Verify the final output with the real tool or consumer.

## Conditional work

Put the decision in plain language:

1. Is this a new skill? Follow the creation workflow.
2. Is it an existing skill? Read its current header and body, then change only the needed part.
3. Does it install files? Package it and use `package_install`; do not copy it into a managed directory directly.
