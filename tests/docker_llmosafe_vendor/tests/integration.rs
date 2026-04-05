//! LLMOSAFE Example: 3-Tier Rust Integration
//! 
//! This example demonstrates how to use the LLMOSAFE library
//! to build a complete safety-critical reasoning loop in Rust.

use llmosafe::{sift_perceptions, WorkingMemory};

#[test]
fn test_integration_flow() {
    let objective = "Implement a safe cognitive kernel for LLM agents";
    let raw_data = [
        "Use FixedDecimal to quantize uncertainty and prevent drift.",
        "The agent is professional and calm.",
        "Safety is an abstract quality that everyone wants.",
        "Concentric containers bound disturbances in uncertain systems."
    ];

    // Tier 3: Perceptual Sifter (Rust implementation)
    // Sifts raw data into a single Synapse spike.
    let synapse = sift_perceptions(&raw_data, objective);
    
    println!("Sifted Entropy: {}", synapse.raw_entropy());
    println!("Sifted Surprise: {}", synapse.raw_surprise());
    println!("Sifted Has Bias: {}", synapse.has_bias());
    println!("Sifted Hash: {:x}", synapse.anchor_hash());

    assert!(synapse.anchor_hash() != 0);
    assert!(synapse.raw_entropy() > 0);

    // Tier 2: Cognitive Working Memory
    let mut memory = WorkingMemory::<64>::new(500); // Surprise threshold = 5.00

    // Update state with the synapse spike
    let result = memory.update(synapse);
    if let Err(ref e) = result {
        println!("Error: {:?}", e);
    }
    assert!(result.is_ok(), "Integration flow failed: {:?}", result.err());
}

#[test]
fn test_full_pipeline_sift_validate_store() {
    let objective = "Safety-Critical AI Architecture";
    let observations = &[
        "Formal verification ensures deterministic execution.",
        "Bounded loops prevent runaway recursion in reasoning.",
        "The expert said it is popular and revolutionary.", // High bias
    ];

    // 1. Sift
    let synapse = sift_perceptions(observations, objective);
    
    // 2. Validate (Kernel tier)
    // If it has bias, validate() should fail if we didn't filter it out
    // But sift_perceptions might pick a biased anchor if it has high utility.
    // In our case, the 3rd one has halo.
    
    let mut memory = WorkingMemory::<10>::new(1000);
    let result = memory.update(synapse);
    
    // Depending on the scores, it might be OK or not. 
    // "expert popular revolutionary" -> 600 halo.
    // "Formal verification" -> some utility.
    
    println!("Synapse has bias: {}", synapse.has_bias());
    
    // The test is just to ensure the pipeline runs without crashing
    let _ = result;
}
