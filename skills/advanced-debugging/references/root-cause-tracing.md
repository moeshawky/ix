# Root Cause Tracing

Bugs often manifest deep in the call stack. Your instinct is to fix where the error appears, but that's treating a symptom.

**Core principle:** Trace backward through the call chain until you find the original trigger, then fix at the source.

## When to Use

- Error happens deep in execution (not at entry point)
- Stack trace shows long call chain
- Unclear where invalid data originated
- Need to find which test/code triggers the problem

## The Tracing Process

### 1. Observe the Symptom
```
Error: git init failed in /Users/jesse/project/packages/core
```

### 2. Find Immediate Cause
```typescript
await execFileAsync('git', ['init'], { cwd: projectDir });
```

### 3. Ask: What Called This?
```typescript
WorktreeManager.createSessionWorktree(projectDir, sessionId)
  -> Session.initializeWorkspace()
  -> Session.create()
  -> test at Project.create()
```

### 4. Keep Tracing Up
- `projectDir = ''` (empty string!)
- Empty string as `cwd` resolves to `process.cwd()`

### 5. Find Original Trigger
```typescript
const context = setupCoreTest(); // Returns { tempDir: '' }
Project.create('name', context.tempDir); // Accessed before beforeEach!
```

## Adding Stack Traces

When manual tracing stalls:

```typescript
async function gitInit(directory: string) {
  const stack = new Error().stack;
  console.error('DEBUG git init:', {
    directory,
    cwd: process.cwd(),
    stack,
  });
  await execFileAsync('git', ['init'], { cwd: directory });
}
```

Use `console.error()` in tests (logger may be suppressed). Log before the dangerous operation, not after failure.

## Finding Test Polluters

If something appears during tests but unknown which test:
- Run tests one-by-one via bisection
- Use `--bail` to stop at first failure
- Add instrumentation before the affected operation

## Key Principle

**NEVER fix just where the error appears.** Trace back to find the original trigger. Then add defense-in-depth at every layer.
