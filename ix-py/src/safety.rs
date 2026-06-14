//! Python bindings for llmosafe's `CognitivePipeline`.
//!
//! Wraps the C-ABI arena (16 slots) to provide a `Pipeline` class that
//! runs text through the full 5-stage cognitive safety pipeline and
//! returns a dictionary of diagnostic fields.

use pyo3::prelude::*;

use crate::error::LLMOSafeError;

/// A cognitive safety pipeline wrapping llmosafe's C-ABI arena.
///
/// Each instance acquires a slot from the 16-slot static arena at
/// construction and releases it on drop.  The pipeline runs text
/// observations through SIFT → MEMORY → KERNEL → DETECTION → MONITOR
/// stages.
///
/// # Lifecycle
///
/// ```python
/// pipe = Pipeline("safety analysis")
/// try:
///     result = pipe.process("user input text")
///     print(result["decision"], result["entropy"])
/// finally:
///     del pipe  # releases the arena slot
/// ```
#[pyclass(name = "Pipeline")]
pub struct PyPipeline {
    handle: usize,
}

/// Helper: call a getter that returns via out-parameter, translate rc to PyResult.
///
/// Return codes from llmosafe v0.7.5:
///   0 = success, 1 = invalid handle, 2 = null pointer, 3 = no result
fn getter_error(name: &str, instance_id: u32, rc: i32) -> PyErr {
    let msg = match rc {
        1 => format!("{name} failed for instance {instance_id}: invalid handle"),
        2 => format!("{name} failed for instance {instance_id}: internal null pointer"),
        3 => format!(
            "{name} failed for instance {instance_id}: no result (sift_and_process not called)"
        ),
        _ => format!("{name} failed for instance {instance_id}: unknown code {rc}"),
    };
    LLMOSafeError::new_err(msg)
}

#[pymethods]
impl PyPipeline {
    /// Create a pipeline with the given safety objective.
    ///
    /// The objective string (max 1024 bytes) defines the drift-detection
    /// target.  Returns an error if all 16 arena slots are occupied.
    #[new]
    fn new(objective: &str) -> PyResult<Self> {
        let handle = llmosafe::c_abi::llmosafe_create(objective.as_ptr(), objective.len());
        if handle == usize::MAX {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "llmosafe arena full — no free pipeline slots",
            ));
        }
        Ok(Self { handle })
    }

    /// Process a text observation through the safety pipeline.
    ///
    /// Returns a dictionary with diagnostic fields:
    ///
    /// * `decision` — safety decision code (0=Proceed, 1=Warn, 2=Escalate, <0=Halt)
    /// * `entropy` — raw entropy value (0–65535)
    /// * `surprise` — raw surprise value (0–65535)
    /// * `detection_flags` — bitmask of active detection signals
    /// * `oov_ratio` — out-of-vocabulary ratio (0–255)
    /// * `stages_executed` — bitmask of stages that ran
    /// * `step_count` — reasoning step count after invocation
    fn process(&mut self, text: &str) -> PyResult<PyObject> {
        let code =
            llmosafe::c_abi::llmosafe_sift_and_process(self.handle, text.as_ptr(), text.len());
        let id = u32::try_from(self.handle)
            .map_err(|_| pyo3::exceptions::PyOverflowError::new_err("pipeline handle overflow"))?;

        // Fetch all 6 getter values using the new out-parameter pattern (v0.7.5).
        let mut entropy: u16 = 0;
        let rc = llmosafe::c_abi::llmosafe_get_entropy(id, &mut entropy);
        if rc != 0 {
            return Err(getter_error("llmosafe_get_entropy", id, rc));
        }

        let mut surprise: u16 = 0;
        let rc = llmosafe::c_abi::llmosafe_get_surprise(id, &mut surprise);
        if rc != 0 {
            return Err(getter_error("llmosafe_get_surprise", id, rc));
        }

        let mut detection_flags: u8 = 0;
        let rc = llmosafe::c_abi::llmosafe_get_detection_flags(id, &mut detection_flags);
        if rc != 0 {
            return Err(getter_error("llmosafe_get_detection_flags", id, rc));
        }

        let mut oov_ratio: u8 = 0;
        let rc = llmosafe::c_abi::llmosafe_get_oov_ratio(id, &mut oov_ratio);
        if rc != 0 {
            return Err(getter_error("llmosafe_get_oov_ratio", id, rc));
        }

        let mut stages_executed: u8 = 0;
        let rc = llmosafe::c_abi::llmosafe_get_stages_executed(id, &mut stages_executed);
        if rc != 0 {
            return Err(getter_error("llmosafe_get_stages_executed", id, rc));
        }

        let mut step_count: u32 = 0;
        let rc = llmosafe::c_abi::llmosafe_get_step_count(id, &mut step_count);
        if rc != 0 {
            return Err(getter_error("llmosafe_get_step_count", id, rc));
        }

        // SAFETY: The GIL is guaranteed held because this method is called
        // from a #[pymethods] context, where pyo3 ensures the GIL is acquired
        // before invoking any bound method.
        let py = unsafe { Python::assume_gil_acquired() };
        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("decision", code)?;
        dict.set_item("entropy", entropy)?;
        dict.set_item("surprise", surprise)?;
        dict.set_item("detection_flags", detection_flags)?;
        dict.set_item("oov_ratio", oov_ratio)?;
        dict.set_item("stages_executed", stages_executed)?;
        dict.set_item("step_count", step_count)?;
        Ok(dict.into())
    }

    /// Returns the safety decision from the last processed observation.
    fn get_decision(&self) -> i32 {
        llmosafe::c_abi::llmosafe_get_decision(self.handle)
    }
}

impl Drop for PyPipeline {
    fn drop(&mut self) {
        llmosafe::c_abi::llmosafe_destroy(self.handle);
    }
}
