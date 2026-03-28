# Activation — Triggers, Rules, and Patterns

One concern: how skills get activated in Claude Code.

## Table of Contents
- [skill-rules.json Schema](#skill-rulesjson-schema)
- [Trigger Types](#trigger-types)
- [Pattern Library](#pattern-library)
- [Testing](#testing)

---

## skill-rules.json Schema

Location: `.claude/skills/skill-rules.json`

```typescript
interface SkillRule {
  type: "guardrail" | "domain";
  enforcement: "block" | "suggest" | "warn";
  priority: "critical" | "high" | "medium" | "low";
  promptTriggers?: {
    keywords?: string[];           // Case-insensitive substring match
    intentPatterns?: string[];     // Regex against user prompt
  };
  fileTriggers?: {
    pathPatterns?: string[];       // Glob against file path
    pathExclusions?: string[];     // Glob exclusions (e.g. test files)
    contentPatterns?: string[];    // Regex against file content
  };
  skipConditions?: {
    sessionTracked?: boolean;      // Don't repeat in same session
    fileMarker?: string;           // Skip if file contains marker
    envVar?: string;               // Skip if env var is set
  };
}
```

**Minimal example:**
```json
{
  "my-skill": {
    "type": "domain",
    "enforcement": "suggest",
    "priority": "medium",
    "promptTriggers": {
      "keywords": ["keyword1", "keyword2"],
      "intentPatterns": ["(create|add).*?something"]
    }
  }
}
```

**Guardrail example (blocking):**
```json
{
  "database-verification": {
    "type": "guardrail",
    "enforcement": "block",
    "priority": "critical",
    "fileTriggers": {
      "pathPatterns": ["**/services/**/*.ts"],
      "contentPatterns": ["import.*[Pp]risma", "PrismaService"],
      "pathExclusions": ["**/*.test.ts", "**/*.spec.ts"]
    },
    "skipConditions": {
      "sessionTracked": true,
      "fileMarker": "// @skip-validation"
    }
  }
}
```

---

## Trigger Types

### Keywords (explicit)
Case-insensitive substring match in user prompt.
- Use specific terms, not generic ("layout" not "system")
- Include variations ("layout", "grid layout", "layout system")

### Intent Patterns (implicit)
Regex matching user's intent without explicit mention.
- Always use `.*?` (non-greedy), never `.*`
- Structure: `(action verbs).*?(domain nouns)`
- Test at regex101.com before deploying

### File Path (location-based)
Glob patterns against file being edited.
- `**` = any depth of directories
- `*` = any characters in one segment
- Always add `pathExclusions` for test files

### Content (technology-specific)
Regex against file contents (imports, class names, API calls).
- Escape special chars: `\\.findMany\\(` not `.findMany(`
- Case-insensitive matching is default

---

## Pattern Library

### Intent patterns (regex)
```
(add|create|implement|build).*?(feature|endpoint|route|service)
(create|add|make|build).*?(component|UI|page|modal|form)
(add|create|modify|update).*?(table|column|schema|migration)
(fix|handle|catch|debug).*?(error|exception|bug)
(how does|explain|what is|describe).*?
(write|create|add).*?(test|spec|unit.*?test)
```

### File path patterns (glob)
```
frontend/src/**/*.tsx         # React components
**/schema.prisma              # Prisma schema anywhere
**/migrations/**/*.sql        # Migrations
**/*.test.ts                  # Test exclusion
```

### Content patterns (regex)
```
import.*[Pp]risma             # Prisma imports
export class.*Controller      # Controller classes
app\.(get|post|put|delete)    # Express routes
useState|useEffect            # React hooks
try\s*\{                      # Try blocks
```

---

## Testing

```bash
# Test prompt triggers (UserPromptSubmit)
echo '{"session_id":"test","prompt":"your test prompt"}' | \
  npx tsx .claude/hooks/skill-activation-prompt.ts

# Test file triggers (PreToolUse)
cat <<'EOF' | npx tsx .claude/hooks/skill-verification-guard.ts
{"session_id":"test","tool_name":"Edit","tool_input":{"file_path":"test.ts"}}
EOF
```

Validate JSON: `jq . .claude/skills/skill-rules.json`
