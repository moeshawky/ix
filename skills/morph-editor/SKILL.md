---
name: morph-editor
description: Guide for using the morph_edit tool to edit files with partial code snippets and markers. Use when editing large files (500+ lines), making multiple scattered changes, complex refactoring, or when you need to make edits without rewriting entire files. Provides syntax guidance for // ... existing code ... markers and best practices for the morph_edit tool.
---

# Morph Editor Skill

Guide for using the `morph_edit` tool correctly with proper syntax and markers.

## When to Use

Use `morph_edit` when:
- Editing files with 500+ lines
- Making multiple scattered changes across a file
- Performing complex refactoring
- Working with partial code snippets
- You want to avoid rewriting entire files

**Prefer `edit` tool for:** Simple, exact string replacements in small files (under 500 lines)

## Key Rules

### CRITICAL: Always Use Markers

**If you omit `// ... existing code ...` markers, Morph will DELETE code.**

**BAD** - will DELETE everything before and after:
```javascript
function newFeature() {
  return "hello";
}
```

**GOOD** - preserves existing code:
```javascript
// ... existing code ...
function newFeature() {
  return "hello";
}
// ... existing code ...
```

**Always wrap your changes with markers at the start AND end** unless you intend to replace the entire file.

## Tool Parameters

1. **target_filepath**: Absolute path to file (use forward slashes, e.g., `D:/path/to/file.js`)
2. **instructions**: First-person description of changes (e.g., "I am adding error handling for null users")
3. **code_edit**: Code snippet with `// ... existing code ...` markers

## Examples

### Example 1: Adding a Function

```javascript
// ... existing code ...
import { newDep } from './newDep';
// ... existing code ...

function newFeature() {
  return newDep.process();
}
// ... existing code ...
```

### Example 2: Modifying Existing Code

```javascript
// ... existing code ...
function existingFunc(param) {
  // Updated implementation
  const result = param * 2; // Changed from * 1
  return result;
}
// ... existing code ...
```

### Example 3: Adding a Timeout

```javascript
// ... existing code ...
export async function fetchData(endpoint) {
  // ... existing code ...
  const response = await fetch(endpoint, {
    headers,
    timeout: 5000  // added timeout
  });
  // ... existing code ...
}
// ... existing code ...
```

### Example 4: Deleting Code

```javascript
// ... existing code ...
function keepThis() {
  return "stays";
}

// The function between these two was removed

function alsoKeepThis() {
  return "also stays";
}
// ... existing code ...
```

## Best Practices

### Provide Context for Disambiguation

When a file has similar code patterns, include enough unique context:

**BAD** - "return result" could match many places:
```javascript
// ... existing code ...
  return result;
}
// ... existing code ...
```

**GOOD** - unique function signature anchors the location:
```javascript
// ... existing code ...
function processUserData(userId) {
  const result = await fetchUser(userId);
  return result;
}
// ... existing code ...
```

### When to Use morph_edit vs Other Tools

| Situation | Tool | Reason |
|-----------|------|--------|
| Small, exact string replacement | `edit` | Fast, precise, no API call |
| Large file (500+ lines) | `morph_edit` | 10x faster, handles partial snippets |
| Multiple scattered changes | `morph_edit` | Batch changes efficiently |
| Complex refactoring | `morph_edit` | Better accuracy with context |
| Whitespace-sensitive edits | `morph_edit` | Forgiving with formatting |
| New file creation | `write` | Standard file creation |

## Common Mistakes

| Mistake | Result | Fix |
|---------|--------|-----|
| No markers at start/end | Deletes code before/after | Always wrap with `// ... existing code ...` |
| Too little context | Wrong location chosen | Add 1-2 unique lines around your change |
| Vague instructions | Ambiguous merge | Be specific: what, where, why |
| Using for tiny changes | Slower than `edit` | Use native `edit` for 1-2 line exact replacements |

## Fallback Behavior

If Morph API fails (timeout, rate limit, etc.):
1. An error message with details is returned
2. Use the native `edit` tool as fallback
3. The native `edit` tool requires exact string matching
