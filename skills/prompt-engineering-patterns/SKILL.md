---
name: prompt-engineering-patterns
description: Master advanced prompt engineering techniques to maximize LLM performance, reliability, and controllability in production. Use when optimizing prompts, improving LLM outputs, or designing production prompt templates.
---

# Prompt Engineering Patterns

> **For multi-agent prompt engineering with failure mode prevention, see `charlie-prompt-engineering`**, which encodes AOP v3 patterns — role-specific templates, anti-pattern injection, model-class-aware construction, and verification gates.
>
> **This skill covers foundational techniques** — few-shot learning, template systems, system prompt design, and optimization workflows. These are the building blocks that `charlie-prompt-engineering` assembles into production-grade, failure-mode-aware patterns.

Master advanced prompt engineering techniques to maximize LLM performance, reliability, and controllability.

---

## When to Use This Skill

- Designing complex prompts for production LLM applications
- Optimizing prompt performance and consistency
- Implementing structured reasoning patterns
- Building few-shot learning systems with dynamic example selection
- Creating reusable prompt templates with variable interpolation
- Debugging and refining prompts that produce inconsistent outputs
- Implementing system prompts for specialized AI assistants

---

## Model-Class Awareness

Prompts must be designed for **capability classes**, not vendor names. The same prompt can produce radically different behavior depending on the class of model receiving it — and each class has a distinct failure profile.

| Class | Strength | Weakness | Design Strategy |
|-------|----------|----------|-----------------|
| **Large Reasoning** (>100B, CoT) | Architectural analysis, planning, decomposition | Over-explains, Minimal-Patch Bias, implements instead of planning | Ask for decomposition and structured output — not implementation |
| **Code-Focused** (tool-calling) | Reliable execution, structured output | Template Fitting (rewrites whole files), stylistic persistence | Atomic operations only. Explicit BANNED list. Never give creative latitude |
| **Reasoning text-only** (analysis) | Deep analysis, high recall, review | May over-flag, poor tool-calling | Text input only. No tools. Structured verdict format |
| **Reliable Instruction** (deterministic) | Precise execution, low hallucination | Misses nuance, interprets too literally | Clear binary decisions. Tie-breaking only. No ambiguity |

**Implication for prompt design**: Before writing a prompt, identify the target model class. Then apply the corresponding design strategy above. A prompt written for a Large Reasoning model will systematically fail if sent to a Code-Focused model, and vice versa.

---

## Failure Mode Prevention

Every prompt you write will trigger one or more failure modes. Design prompts to suppress the likely failures for the target model class. See `charlie-prompt-engineering` for the full 9-mode taxonomy — the modes most relevant to foundational prompt design are:

| Code | What Goes Wrong | Prompt Prevention |
|------|----------------|-------------------|
| G-HALL | Invented APIs / methods that don't exist | Provide exact imports and signatures. Never say "use the appropriate method." |
| G-EDGE | Missing edge cases (null, empty, boundary, unicode) | List edge cases explicitly. "Handle edge cases" is not a prompt constraint. |
| G-SEM | Output looks right but behaves wrong | Add a behavioral test: "After this, X should still return Y." |
| G-ERR | Happy path only — no error handling | Ask explicitly: "What happens when [input] is null/empty/malformed?" |
| G-CTX | Works alone, fails when integrated | Provide blast radius context. Show callers. Don't let the model predict consequences. |
| G-DRIFT | Same prompt, different output across model versions | Use structured output (YAML/JSON). Pin the expected format. |

Use these as **prompt design constraints**, not as post-hoc debugging hints. When 2+ failure modes appear in output, the prompt itself is wrong — don't patch, rewrite with concrete examples.

---

## Structured Reasoning: `productive_reason` over Ad-Hoc CoT

For structured reasoning tasks, use the **`productive_reason` MCP tool** (SEE → EXPLORE → CONVERGE → REFLECT) instead of ad-hoc chain-of-thought prompting.

**Why**: "Let's think step by step" produces unstructured reasoning that is hard to validate, audit, or reproduce. The `productive_reason` tool enforces a four-phase structure:

- **SEE**: What is actually present? What evidence exists?
- **EXPLORE**: What are the competing interpretations or approaches?
- **CONVERGE**: Which option is best and why?
- **REFLECT**: What could still go wrong?

When you need to prompt a model to reason (rather than execute), structure the prompt to mirror these four phases explicitly rather than leaving reasoning open-ended:

```
Phase 1 — SEE: List what you observe in the following [diff / error / code]:
Phase 2 — EXPLORE: What are the 2-3 possible interpretations?
Phase 3 — CONVERGE: Which interpretation is best supported by the evidence?
Phase 4 — REFLECT: What could invalidate your conclusion?
```

This replaces zero-shot CoT ("Let's think step by step") and self-consistency sampling for most production use cases.

---

## Core Capabilities

### 1. Few-Shot Learning

Few-shot examples are the most reliable way to communicate intent — more reliable than instructions alone. Examples show the model what "correct" means without ambiguity.

- **Example selection strategies**: semantic similarity (retrieve examples close to the current input), diversity sampling (cover the edge-case space), adversarial selection (include a failure case to show what NOT to do)
- **Balance example count with context**: 2-3 strong examples usually outperform 8-10 mediocre ones
- **Construct demonstrations with full input-output pairs**: partial examples confuse more than they help
- **Dynamic example retrieval**: retrieve from a knowledge base using the current input as the query
- **Handle edge cases through examples**: if you need the model to handle null input gracefully, show it handling null input

```python
from prompt_optimizer import PromptTemplate, FewShotSelector

template = PromptTemplate(
    system="You are an expert SQL developer. Generate efficient, secure SQL queries.",
    instruction="Convert the following natural language query to SQL:\n{query}",
    few_shot_examples=True,
    output_format="SQL code block with explanatory comments"
)

selector = FewShotSelector(
    examples_db="sql_examples.jsonl",
    selection_strategy="semantic_similarity",
    max_examples=3
)

prompt = template.render(
    query="Find all users who registered in the last 30 days",
    examples=selector.select(query="user registration date filter")
)
```

### 2. Prompt Optimization

- Iterative refinement: change one variable at a time
- A/B testing prompt variations against a fixed evaluation set
- Measuring accuracy, consistency, and latency
- Reducing token usage without sacrificing quality: move stable content to system prompts, remove redundant instructions
- Edge case coverage: test on unusual, boundary, and adversarial inputs before shipping

### 3. Template Systems

Templates are the unit of reuse in prompt engineering. A good template:
- Isolates what varies (user input, retrieved context, examples) from what is stable (role, constraints, output format)
- Uses explicit variable interpolation: `{query}`, `{context}`, `{examples}` — never string concatenation
- Has conditional sections for optional context (e.g., include examples only if available)
- Composes modular components: system context + task instruction + examples + input + output format

### 4. System Prompt Design

System prompts set the invariants — constraints that apply to every turn in a conversation.

- Define role and expertise at the top
- State output format requirements (JSON, YAML, structured prose) — models follow format constraints reliably
- Include explicit BANNED behaviors for the target model class (see `charlie-prompt-engineering` BANNED section templates)
- Add safety guidelines and content policies that apply to the domain
- Keep system prompts stable — they are cached; frequent edits defeat prefix caching

---

## Key Patterns

### Progressive Disclosure

Start with simple prompts, add complexity only when needed:

1. **Level 1**: Direct instruction — "Summarize this article"
2. **Level 2**: Add constraints — "Summarize this article in 3 bullet points, focusing on key findings"
3. **Level 3**: Add reasoning structure — "Read this article, identify the main findings, then summarize in 3 bullet points"
4. **Level 4**: Add examples — Include 2-3 example summaries with input-output pairs

### Instruction Hierarchy

```
[System Context] → [Task Instruction] → [Examples] → [Input Data] → [Output Format]
```

This order matters. Models weight earlier content more heavily for behavioral constraints (system context, role) and use later content as data to process (input, examples).

### Error Recovery

Build prompts that gracefully handle failures:
- Include fallback instructions for when data is missing or ambiguous
- Request confidence scores when the model should express uncertainty
- Ask for alternative interpretations when the input is underspecified
- Specify how to indicate missing information (e.g., "Return `null` in the field, not a guess")

---

## Best Practices

1. **Be Specific**: Vague prompts produce inconsistent results — inconsistency is usually a specificity problem
2. **Show, Don't Tell**: Examples communicate intent more reliably than descriptions
3. **Test Extensively**: Evaluate on diverse, representative, and adversarial inputs
4. **Iterate Rapidly**: Small changes can have large impacts — change one variable at a time
5. **Monitor in Production**: Prompt performance degrades as model versions change (G-DRIFT)
6. **Version Control**: Treat prompts as code — diff them, review them, track changes
7. **Document Intent**: Explain WHY the prompt is structured as it is, not just what it does
8. **Target the model class**: Never write a prompt for "the model" — write it for the capability class

---

## Common Pitfalls

- **Over-engineering**: Starting with complex prompts before trying simple ones
- **Example pollution**: Using examples that don't match the target task distribution
- **Context overflow**: Exceeding token limits with excessive or low-quality examples
- **Ambiguous instructions**: Leaving room for multiple valid interpretations
- **Ignoring edge cases**: Not testing on unusual or boundary inputs before shipping
- **Ad-hoc chain-of-thought**: Unstructured "think step by step" is hard to audit — use `productive_reason` for structured reasoning tasks

---

## Integration Patterns

### With RAG Systems

```python
prompt = f"""Given the following context:
{retrieved_context}

{few_shot_examples}

Question: {user_question}

Provide a detailed answer based solely on the context above. If the context doesn't contain enough information, explicitly state what's missing."""
```

### With Validation

```python
prompt = f"""{main_task_prompt}

After generating your response, verify it meets these criteria:
1. Answers the question directly
2. Uses only information from provided context
3. Cites specific sources
4. Acknowledges any uncertainty

If verification fails, revise your response."""
```

---

## Performance Optimization

### Token Efficiency
- Remove redundant words and phrases
- Use abbreviations consistently after first definition
- Consolidate similar instructions
- Move stable content to system prompts (prefix caching)

### Latency Reduction
- Minimize prompt length without sacrificing quality
- Use streaming for long-form outputs
- Cache common prompt prefixes
- Batch similar requests when possible

---

## Resources

- **`charlie-prompt-engineering`** — production patterns: role-specific templates, AOP v3 failure mode prevention, anti-pattern injection, verification gates
- **references/few-shot-learning.md**: Deep dive on example selection and construction
- **references/chain-of-thought.md**: Advanced reasoning elicitation techniques
- **references/prompt-optimization.md**: Systematic refinement workflows
- **references/prompt-templates.md**: Reusable template patterns
- **references/system-prompts.md**: System-level prompt design
- **assets/prompt-template-library.md**: Battle-tested prompt templates
- **assets/few-shot-examples.json**: Curated example datasets
- **scripts/optimize-prompt.py**: Automated prompt optimization tool

---

## Success Metrics

Track these KPIs for your prompts:

- **Accuracy**: Correctness of outputs against ground truth
- **Consistency**: Reproducibility across similar inputs and across model versions (G-DRIFT)
- **Failure mode rate**: How often each of the 9 AOP modes appears in output
- **Latency**: Response time (P50, P95, P99)
- **Token Usage**: Average tokens per request
- **Edge case coverage**: Percentage of boundary inputs handled correctly

---

## Next Steps

1. Read `charlie-prompt-engineering` if you are writing prompts for multi-agent work — it encodes the full AOP v3 protocol
2. Review the prompt template library for common patterns
3. Identify the target model class before writing any prompt
4. Run the failure mode checklist against your prompt before shipping
5. Implement prompt versioning and A/B testing against a fixed evaluation set
6. Set up automated evaluation pipelines with structured output validation
