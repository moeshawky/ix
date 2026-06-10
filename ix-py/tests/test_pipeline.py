"""Tests for the Pipeline class — C-PIPELINE contract.

Pipeline wraps llmosafe's C-ABI arena (16 slots) to provide cognitive safety
filtering via a 5-stage pipeline.  Each Pipeline instance acquires a slot at
construction and releases it on drop.

Contract fields returned by process():
    decision        — i32 safety decision code (0=Proceed, 1=Warn, 2=Escalate, <0=Halt)
    entropy         — u16 raw entropy value (0–65535)
    surprise        — u16 raw surprise value (0–65535)
    detection_flags — u16 bitmask of active detection signals
    oov_ratio       — u8 out-of-vocabulary ratio (0–255)
    stages_executed — u8 bitmask of stages that ran
    step_count      — u64 reasoning step count after invocation
"""

from __future__ import annotations

import ix

# ---------------------------------------------------------------------------
# All 7 keys that process() is guaranteed to return (see ix-py/src/safety.rs:69-82)
# ---------------------------------------------------------------------------
_PROCESS_KEYS = frozenset(
    [
        "decision",
        "entropy",
        "surprise",
        "detection_flags",
        "oov_ratio",
        "stages_executed",
        "step_count",
    ]
)

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_pipeline_create() -> None:
    """Pipeline can be constructed with a string objective and is not None."""
    pipe = ix.Pipeline("safety analysis")
    assert pipe is not None


def test_pipeline_process() -> None:
    """process() returns a dict containing all 7 contract fields."""
    pipe = ix.Pipeline("safety check")
    result = pipe.process("Analyze this text for safety concerns.")
    assert isinstance(result, dict)
    # Every contract field must be present
    assert _PROCESS_KEYS.issubset(result.keys())
    # All contract values are integers
    for key in _PROCESS_KEYS:
        assert isinstance(result[key], int), f"'{key}' expected int, got {type(result[key]).__name__}"


def test_pipeline_get_decision() -> None:
    """get_decision() returns an int even when called before any process()."""
    pipe = ix.Pipeline("pre-process decision check")
    decision = pipe.get_decision()
    assert isinstance(decision, int)


def test_pipeline_process_then_decision() -> None:
    """get_decision() matches the 'decision' field returned by the last process() call."""
    pipe = ix.Pipeline("consistency check")
    result = pipe.process("Verify decision consistency across methods.")
    from_dict = result["decision"]
    from_method = pipe.get_decision()
    assert from_dict == from_method, (
        f"process()['decision']={from_dict} but get_decision()={from_method}"
    )
    assert isinstance(from_dict, int)


def test_pipeline_empty_text() -> None:
    """process() with an empty string must not crash and still returns a dict."""
    pipe = ix.Pipeline("empty input check")
    result = pipe.process("")
    assert isinstance(result, dict)
    assert _PROCESS_KEYS.issubset(result.keys())


def test_pipeline_different_objectives() -> None:
    """Multiple pipelines with different objectives can coexist in the arena."""
    pipe_a = ix.Pipeline("content moderation")
    pipe_b = ix.Pipeline("code review safety")

    res_a = pipe_a.process("Some user-generated content.")
    res_b = pipe_b.process("fn main() { println!(\"hello\"); }")

    assert isinstance(res_a, dict)
    assert isinstance(res_b, dict)
    assert _PROCESS_KEYS.issubset(res_a.keys())
    assert _PROCESS_KEYS.issubset(res_b.keys())
