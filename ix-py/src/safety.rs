//! Python bindings for llmosafe's `CognitivePipeline`.
//!
//! Wraps the C-ABI arena (16 slots) to provide a `Pipeline` class that
//! runs text through the full 5-stage cognitive safety pipeline and
//! returns a dictionary of diagnostic fields.

use pyo3::prelude::*;

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
    #[allow(clippy::as_conversions)]
    fn process(&mut self, text: &str) -> PyResult<PyObject> {
        let code =
            llmosafe::c_abi::llmosafe_sift_and_process(self.handle, text.as_ptr(), text.len());
        let id = self.handle as u32;
        // SAFETY: The GIL is guaranteed held because this method is called
        // from a #[pymethods] context, where pyo3 ensures the GIL is acquired
        // before invoking any bound method.
        let py = unsafe { Python::assume_gil_acquired() };
        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("decision", code)?;
        dict.set_item("entropy", llmosafe::c_abi::llmosafe_get_entropy(id))?;
        dict.set_item("surprise", llmosafe::c_abi::llmosafe_get_surprise(id))?;
        dict.set_item(
            "detection_flags",
            llmosafe::c_abi::llmosafe_get_detection_flags(id),
        )?;
        dict.set_item("oov_ratio", llmosafe::c_abi::llmosafe_get_oov_ratio(id))?;
        dict.set_item(
            "stages_executed",
            llmosafe::c_abi::llmosafe_get_stages_executed(id),
        )?;
        dict.set_item("step_count", llmosafe::c_abi::llmosafe_get_step_count(id))?;
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
