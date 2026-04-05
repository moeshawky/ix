### AGENTIC README STANDARD V2.0: ARCHITECTURAL CONTRACT FOR LLM ORCHESTRATION

## I. MISSION MANDATE: Context Compaction and Bounded Reasoning

As the system architect, I mandate that this document serves as the formal **external contract** of the module. Its primary mission is to ensure **optimal thinking spend** by translating human intent into concise, structured inputs consumable by the LLM Planner ("The Brain").

This contract defines the constraints necessary for the centralized orchestrator (The Brain, operating under the Puppeteer Policy) to execute complex goals via efficient, short-context, **Iterative Conversational Collaboration (ICCM)** or **Planning-Driven Models (PDM)**.

The structure prioritizes **Context Compaction** by providing the smallest possible set of high-signal semantic primitives, preventing the model from struggling with long-context failure modes.

## II. CORE PRINCIPLES (ALIGNMENT WITH VIBE CODING)

This documentation must align with the contemporary paradigm of Vibe Coding, where the developer's role shifts from author to orchestrator and quality arbiter.

### 1. The Principle of Outcome Observation
The correctness of AI-generated implementations must be verifiable through **outcome observation** (e.g., successful test execution, system state change) rather than line-by-line code comprehension. The README must define clear criteria for these observable outcomes.

### 2. The Principle of Sufficient Abstraction
The document must fully abstract away internal implementation logic. An agent must successfully utilize the module's public interface **without requiring examination of the source code**, facilitating modularity (Orthogonality) and high-velocity development.

### 3. The Principle of Bounded Workspace ($C_t$)
Content must be synthesized for **high-signal density**. Narrative repetition and redundancy are forbidden (DRY). Every section must contribute explicitly to defining the bounded project context space $P = \langle C_{code}, C_{data}, C_{know} \rangle$.

## III. REQUIRED STRUCTURE (AGENT COGNITIVE FUNNELING)

The document structure follows a **Cognitive Funneling** pattern, prioritizing immediate constraint filtering (short-circuiting) for the orchestrator, and aligning with the necessary input schema for the **Planning-Driven Model (PDM)**.

### A. IDENTIFICATION AND HIGH-LEVEL CONTRACT

| Element | Purpose for Agent Orchestration | Constraint Mandate |
| :--- | :--- | :--- |
| **Name** | Unambiguous module identifier. | Must resolve quickly against the agent's internal tool registry. |
| **One-Liner** | Defines the exact problem domain/functionality. | Provides immediate semantic context for the orchestrator's initial intent articulation. |
| **Dependencies** | External requirements or libraries (Rust crates, protocols). | Must be listed explicitly to inform the Planner Agent of necessary initialization steps. |
| **License** | Defines usage constraints. | Must be checked early for compliance validation by the Kernel. |

### B. FUNCTIONAL INTEGRATION & CORE EXAMPLES

| Element | Purpose for Agent Workflow | Constraint Mandate |
| :--- | :--- | :--- |
| **Usage** | Canonical code snippet demonstrating basic function sequencing. | Must enable the agent to rapidly iterate and refine intention through conversational feedback (ICCM). |
| **Installation** | Setup steps for local persistence and dependencies. | Must include non-standard environmental setup (e.g., Docker, DB migrations). |

### C. UTCP API SPECIFICATION (Tool Schema)

This is the non-negotiable definition of the module's interaction contract. It must expose capabilities explicitly as executable tools conforming to the **Universal Tool Calling Protocol (UTCP)** for low-latency execution.

1.  **Tool/Function List:** Enumeration of all exposed methods.
2.  **Schema:** Formal JSON schema defining input parameters, required types, and explicit return types (necessary for accurate LLM parameter generation).
3.  **Constraints:** Explicitly state **Hard Constraints** applied by the Kernel (e.g., resource limits, atomic operations).

### D. ARCHITECTURAL CONSTRAINTS & STATE

This section defines environmental facts that the agent **must not** attempt to modify or ignore, serving as necessary constraints in the planning process.

1.  **Immutables:** Define read-only system facts (e.g., database type, authentication scheme, API version).
2.  **Architectural Context:** Explicitly define where this module fits in the $H, P, A_{\theta}$ triad and the execution flow (e.g., "This runs asynchronously via the Event Bus" or "This executes within the isolated execution runtime".
3.  **Context Momentum:** Summarize necessary session state variables that must persist across iterative calls (e.g., `session_id`, `token_id`) to prevent task degradation or repetition.

## IV. VALIDATION AND ADAPTIVE EVOLUTION

### 1. Outcome Validation Mandate
Validation must be centered around verifiable outcomes, consistent with the Test-Driven Model (TDM) philosophy adapted for Vibe Coding.

*   **Doctests/Unit Tests:** All usage examples must be packaged as runnable Doctests.
*   **Success Criteria:** Define objective metrics (e.g., final answer correctness, compile status, latency below threshold, security score) that serve as feedback signals for the agent's self-improvement.
*   **Behavioral Diffs:** Define acceptable transformations of the codebase (e.g., expected file outputs or database schema changes) to validate the agent’s actions against human intent.

### 2. Adaptive Evolution
This module supports the RL policy optimization (Adaptive Evolution) that maximizes the solution quality while minimizing computational cost $\lambda$.

*   **Telemetry Required:** Must log UTCP calls, input/output tokens, and latency for the Kernel to compute the objective reward.
*   **Learned SOPs:** Document the location where successful, reusable sequences (Standard Operating Procedures/SOPs) related to this module are stored (Acontext Space). The Experience Agent uses these artifacts to reduce future planning load.