# Contract-First Design

Every boundary in a system is a contract. Unspecified boundaries are bugs waiting for integration time.

## The Contract Principle

A boundary is defined when it has ALL of:
1. **Input types** — What goes in (with constraints)
2. **Output types** — What comes back (success AND error shapes)
3. **Error types** — How failures are communicated
4. **Invariants** — What is always true (pre/post conditions)

If any of these four is described in prose instead of types, the contract is incomplete.

## API Contract Design

### REST Endpoints

Minimum specification per endpoint:

```
METHOD /path/{param}
  Path params:   { param: string (uuid) }
  Query params:  { limit?: number (1-100, default 20), cursor?: string }
  Request body:  { field: Type, field2?: Type }
  Response 200:  { data: Type[], cursor?: string }
  Response 400:  { error: string, details: ValidationError[] }
  Response 404:  { error: string }
  Response 500:  { error: string, requestId: string }
```

### Versioning Strategy

Choose ONE:
- **URL versioning** (`/v1/resource`) — Simple, explicit, works with any client
- **Header versioning** (`Accept: application/vnd.api.v1+json`) — Cleaner URLs, harder to test
- **No versioning** — Acceptable for internal APIs with coordinated deploys

**Default:** URL versioning. It's boring and it works.

### Pagination

Choose ONE:
- **Cursor-based** — For feeds, timelines, real-time data. No count, no skip.
- **Offset-based** — For admin panels, reports. Allows jumping to page N. Breaks with concurrent writes.
- **Keyset** — For sorted datasets. Efficient at scale. Requires stable sort key.

**Default:** Cursor-based. Offset only when users need to jump to specific pages.

### Error Response Contract

Every API must return errors in a consistent shape:

```typescript
interface ErrorResponse {
  error: string;           // Machine-readable error code (e.g., "VALIDATION_FAILED")
  message: string;         // Human-readable description
  details?: unknown[];     // Structured error details (validation errors, etc.)
  requestId: string;       // For tracing/debugging
}
```

Rules:
- Never return stack traces in production
- Error codes are stable (clients depend on them) — document them
- HTTP status codes are coarse categories, error codes are specific
- 4xx = client's fault (can retry with different input), 5xx = server's fault (retry same request)

## Schema Design

### Database Schema Contracts

Each table/collection specifies:

```
Table: orders
  Columns:
    id          UUID        PK, generated
    user_id     UUID        FK(users.id), NOT NULL, indexed
    status      ENUM        ('pending','confirmed','shipped','delivered','cancelled')
    total_cents INTEGER     NOT NULL, CHECK(>= 0)
    created_at  TIMESTAMPTZ NOT NULL, DEFAULT now()
    updated_at  TIMESTAMPTZ NOT NULL, DEFAULT now()

  Constraints:
    - status transitions: pending -> confirmed -> shipped -> delivered
    - status transitions: pending -> cancelled, confirmed -> cancelled
    - total_cents is immutable after status = 'confirmed'

  Indexes:
    - (user_id, created_at DESC) — user's recent orders
    - (status) WHERE status IN ('pending','confirmed') — active orders
```

### State Machine Contracts

Any field with restricted transitions is a state machine. Document:
1. All valid states
2. All valid transitions (from → to)
3. Who/what triggers each transition
4. Side effects of each transition

```
pending ──► confirmed ──► shipped ──► delivered
   │            │
   └──► cancelled ◄──┘
```

### Event/Message Schema Contracts

```typescript
interface OrderPlacedEvent {
  type: "order.placed";        // Discriminator
  version: 1;                  // Schema version
  timestamp: string;           // ISO 8601
  data: {
    orderId: string;
    userId: string;
    items: Array<{
      productId: string;
      quantity: number;        // > 0
      unitPriceCents: number;  // >= 0
    }>;
    totalCents: number;        // Sum of (quantity * unitPriceCents)
  };
}
```

Rules:
- Every event has a type discriminator and version
- Consumers MUST ignore unknown fields (forward compatibility)
- Producers MUST NOT remove fields without version bump (backward compatibility)
- New required fields = new version

## Interface Contracts Between Modules

### Port/Adapter Interfaces

```typescript
// Port (owned by domain layer)
interface PaymentGateway {
  charge(amount: Money, method: PaymentMethod): Promise<ChargeResult>;
  refund(chargeId: string, amount?: Money): Promise<RefundResult>;
}

// ChargeResult is a discriminated union — caller MUST handle both cases
type ChargeResult =
  | { status: "success"; chargeId: string; settledAt: Date }
  | { status: "declined"; reason: string; retryable: boolean };
```

Rules:
- Interfaces owned by the CONSUMER, not the provider
- Return types are discriminated unions, not exceptions for expected outcomes
- Exceptions only for unexpected infrastructure failures

### Dependency Rules

```
Module A ──depends on──► Interface (Port)
                              ▲
                              │ implements
Module B (Adapter) ───────────┘
```

- A depends on the interface, never on B directly
- B implements the interface defined by A
- If A needs to change the interface, B must adapt
- If B needs capabilities not in the interface, extend the interface (don't bypass it)

## Contract Testing

### Consumer-Driven Contracts

1. Consumer writes a test: "I call endpoint X with Y, I expect Z"
2. Provider runs consumer's tests in their CI
3. If provider breaks a consumer test, the build fails

This catches breaking changes BEFORE deployment, not after.

### Schema Validation

- API: OpenAPI/JSON Schema validation middleware
- Events: JSON Schema or Avro schema registry
- Database: Migration-tested constraints
- Config: Typed config objects validated at startup (fail fast)

## Contract Checklist

Before declaring a boundary "designed":

- [ ] Input types with constraints specified
- [ ] Output types for ALL status codes specified
- [ ] Error shape is consistent across the API
- [ ] Pagination strategy chosen and documented
- [ ] State machines have transition diagrams
- [ ] Events have type discriminators and version fields
- [ ] Backward/forward compatibility rules stated
- [ ] Contract tests exist or are planned
