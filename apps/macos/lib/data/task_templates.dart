// Project-neutral task templates; verified against assets/task_templates in tests.
const taskIssueKinds = ['Bug', 'Feature', 'Custom'];
const taskTemplates = <String, String>{
  'Bug': r'''# Bug report

## What happened?

## Expected behavior

## Steps to reproduce
1.
2.
3.

## Environment
Project version / commit:
Operating system:
Relevant tools / versions:

## Affected area

## Logs or error output

```text
```

## Screenshots / recordings

## Additional context
''',
  'Feature': r'''# Feature request

## Problem to solve
Describe the user need and the current limitation.

## Proposed behavior
Describe what the feature should do.

## Scope
Included:
Out of scope:

## Acceptance criteria
- [ ] 

## Alternatives considered

## Affected area

## Examples / screenshots

## Additional context
''',
  'Custom': r'''# Task

## Description

Describe what needs to be done.

## Expected outcome

## Scope

## Acceptance criteria
- [ ]

## Additional context
''',
};
