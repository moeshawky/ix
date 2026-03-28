# Content Patterns — Writing Effective Skill Content

One concern: patterns for structuring what goes INSIDE a skill.

## Sequential Workflows

Break complex tasks into clear steps with an overview upfront:

```markdown
Filling a PDF form involves these steps:
1. Analyze the form (run analyze_form.py)
2. Create field mapping (edit fields.json)
3. Validate mapping (run validate_fields.py)
4. Fill the form (run fill_form.py)
5. Verify output (run verify_output.py)
```

## Conditional Workflows

Guide through decision points:

```markdown
1. Determine the modification type:
   **Creating new content?** → Follow "Creation workflow" below
   **Editing existing content?** → Follow "Editing workflow" below

2. Creation workflow: [steps]
3. Editing workflow: [steps]
```

## Output Templates

### Strict (API responses, data formats)

```markdown
## Report structure

ALWAYS use this exact template:

# [Analysis Title]
## Executive summary
[One-paragraph overview]
## Key findings
- Finding 1 with data
## Recommendations
1. Specific actionable item
```

### Flexible (when adaptation is useful)

```markdown
## Report structure

Sensible default — adapt as needed:

# [Analysis Title]
## Executive summary
[Overview]
## Key findings
[Adapt sections to what you discover]
```

## Input/Output Examples

For output quality that depends on seeing the pattern:

```markdown
**Example 1:**
Input: Added user authentication with JWT tokens
Output: feat(auth): implement JWT-based authentication

**Example 2:**
Input: Fixed bug where dates displayed incorrectly
Output: fix(reports): correct date formatting in timezone conversion
```

Examples beat descriptions. Show the pattern, don't explain it.
