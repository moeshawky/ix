# Git Commit Message Rules

## Format
```
<subject line>

<body>

<metadata>
```

## Subject Line Rules
1. **Max 50 chars** (hard limit: 72)
2. **Capitalize** first letter
3. **No period** at end
4. **Use imperative mood** — write as command (e.g., "Add feature" not "Added feature")
5. **Test**: Subject should complete the sentence: *"If applied, this commit will ___"*

## Body Rules
1. **Separate from subject** with blank line
2. **Wrap at 72 chars**
3. **Explain WHAT and WHY**, not how (code shows how)
4. **Optional** for trivial changes

## Metadata
- Place issue/PR references at bottom
- Format: `Resolves: #123` or `See also: #456`

## Examples

**Simple commit:**
```
Fix typo in user guide introduction
```

**With body:**
```
Add caching layer to user authentication

The previous implementation queried the database on every request,
causing significant latency under load. This adds Redis-based session
caching to reduce average auth time from 200ms to 15ms.

Resolves: #234
```

## Quick Checklist
- [ ] Subject ≤50 chars, imperative, capitalized, no period
- [ ] Blank line before body (if body exists)
- [ ] Body wrapped at 72 chars
- [ ] Body explains why, not how
- [ ] References at bottom