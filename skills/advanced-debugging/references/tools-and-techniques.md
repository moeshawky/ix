# Debugging Tools and Techniques

## JavaScript/TypeScript

### Chrome DevTools / Node Debugger

```typescript
// Pause execution
debugger;

// Conditional breakpoint
if (order.items.length > 10) { debugger; }

// Console techniques
console.log('Value:', value);
console.table(arrayOfObjects);
console.time('op'); /* code */ console.timeEnd('op');
console.trace();  // stack trace
console.assert(value > 0, 'Must be positive');

// Performance marks
performance.mark('start');
// ... operation
performance.mark('end');
performance.measure('operation', 'start', 'end');
```

### VS Code Launch Config

```json
{
  "type": "node",
  "request": "launch",
  "name": "Debug",
  "program": "${workspaceFolder}/src/index.ts",
  "outFiles": ["${workspaceFolder}/dist/**/*.js"],
  "skipFiles": ["<node_internals>/**"]
}
```

### Memory Leak Detection

```typescript
if (process.memoryUsage().heapUsed > 500 * 1024 * 1024) {
  console.warn('High memory:', process.memoryUsage());
  require('v8').writeHeapSnapshot();
}
```

## Python

### pdb / ipdb

```python
import pdb; pdb.set_trace()    # Classic
breakpoint()                     # Python 3.7+
from ipdb import set_trace; set_trace()  # Better UI

# Post-mortem
try:
    risky_operation()
except Exception:
    import pdb; pdb.post_mortem()
```

### Profiling

```python
import cProfile, pstats
cProfile.run('slow_function()', 'stats')
stats = pstats.Stats('stats')
stats.sort_stats('cumulative')
stats.print_stats(10)
```

### Logging

```python
import logging
logging.basicConfig(level=logging.DEBUG)
logger = logging.getLogger(__name__)
logger.debug(f'Fetching user: {user_id}')
```

## Go

### Delve

```bash
go install github.com/go-delve/delve/cmd/dlv@latest
dlv debug main.go
```

```go
import "runtime/debug"
debug.PrintStack()  // Print stack trace

// Panic recovery
defer func() {
    if r := recover(); r != nil {
        fmt.Println("Panic:", r)
        debug.PrintStack()
    }
}()

// pprof
import _ "net/http/pprof"
// Visit http://localhost:6060/debug/pprof/
```

## Git Bisect

```bash
git bisect start
git bisect bad                # Current commit broken
git bisect good v1.0.0        # Known good
# Test middle commit, then:
git bisect good  # or  git bisect bad
# Repeat until found
git bisect reset
```

## Differential Debugging

| Aspect | Working | Broken |
|--------|---------|--------|
| Environment | Dev | Prod |
| Runtime version | 18.16.0 | 18.15.0 |
| Data | Empty DB | 1M records |
| User | Admin | Regular |
| Time | Daytime | After midnight |

## Intermittent Bugs

1. Add extensive logging (timing, state transitions, external calls)
2. Look for race conditions (shared state, async ordering, missing locks)
3. Check timing dependencies (setTimeout, promise order, animation frames)
4. Stress test (many iterations, varied timing, simulated load)

## Performance Issues

1. Profile first - don't optimize blindly
2. Common culprits: N+1 queries, unnecessary re-renders, large data, sync I/O
3. Tools: Chrome DevTools Performance, Lighthouse, cProfile, clinic.js

## Production Debugging

1. Gather evidence (Sentry/Bugsnag, logs, user reports, metrics)
2. Reproduce locally with production data (anonymized)
3. Don't change production - use feature flags, staging, monitoring
