//! LLMOSAFE Tier 3 Perceptual Sifter (Rust Implementation)
//! 
//! This module implements the "Perceptual Decoupling" Axiom.
//! It acts as a 'Thinking Proxy' (MemSifter pattern) that sifts external
//! reality into high-signal anchors for the Tier 1/2 safety core.
//!
//! Research Grounds:
//! - MemSifter: Think-and-Rank proxy reasoning.
//! - Selective Attention: Bias/Halo screening (SCS).
//! - GDWM: Task-Contextual Utility (CPMI).

use crate::llmosafe_kernel::Synapse;
use std::collections::HashMap;

/// Authority Bias Keywords: Red flags for expertise/position bias.
pub const AUTHORITY_BIAS: &[&str] = &[
    "expert", "official", "government", "doctor", "professor", "scientist", "research", "study", "guaranteed", "certified", 
    "authorized", "proven", "reliable", "trusted", "professional", "specialist", "veteran", "master", "guru", "authority"
];

/// Social Proof Keywords: Red flags for crowd/popularity bias.
pub const SOCIAL_PROOF: &[&str] = &[
    "popular", "everyone", "thousands", "millions", "trending", "bestseller", "viral", "community", "joined", "users", 
    "testimonials", "reviews", "ratings", "common", "standard", "consensus", "majority", "crowd", "peer", "social"
];

/// Scarcity Keywords: Red flags for restricted availability bias.
pub const SCARCITY: &[&str] = &[
    "limited", "rare", "exclusive", "only", "few", "unique", "special", "handcrafted", "small-batch", "collectible", 
    "once-in-a-lifetime", "select", "restricted", "shortage", "vanishing", "low-stock", "while-supplies-last", "sold-out", "member-only", "private"
];

/// Urgency Keywords: Red flags for time-pressure bias.
pub const URGENCY: &[&str] = &[
    "now", "today", "fast", "quick", "instant", "hurry", "rush", "deadline", "expiring", "ending", 
    "soon", "immediately", "pronto", "rapid", "speedy", "limited-time", "last-chance", "act-now", "don't-wait", "final"
];

/// Emotional Appeal Keywords: Red flags for emotional manipulation bias.
pub const EMOTIONAL_APPEAL: &[&str] = &[
    "love", "joy", "fear", "worry", "happy", "sad", "angry", "exciting", "amazing", "beautiful", 
    "heartwarming", "shocking", "miracle", "incredible", "tragic", "desperate", "hopeful", "passionate", "inspiring", "emotional"
];

/// Expertise Signaling Keywords: Red flags for jargon/complexity bias.
pub const EXPERTISE_SIGNALING: &[&str] = &[
    "sophisticated", "advanced", "cutting-edge", "state-of-the-art", "revolutionary", "innovative", "patented", "breakthrough", "proprietary", "complex", 
    "technical", "paradigm", "holistic", "synergy", "leverage", "optimize", "agile", "lean", "scalable", "high-performance"
];

/// Returns a breakdown of detected biases by category.
pub fn get_bias_breakdown(text: &str) -> HashMap<&str, u16> {
    let mut breakdown = HashMap::new();
    let text_lower = text.to_lowercase();
    
    let mut calculate_cat = |name: &'static str, keywords: &[&str]| {
        let mut score = 0;
        for keyword in keywords {
            if text_lower.contains(keyword) {
                score += 200;
            }
        }
        breakdown.insert(name, score);
    };

    calculate_cat("authority_bias", AUTHORITY_BIAS);
    calculate_cat("social_proof", SOCIAL_PROOF);
    calculate_cat("scarcity", SCARCITY);
    calculate_cat("urgency", URGENCY);
    calculate_cat("emotional_appeal", EMOTIONAL_APPEAL);
    calculate_cat("expertise_signaling", EXPERTISE_SIGNALING);

    breakdown
}

/// Calculate the "Halo Signal" (SCS Proxy).
/// Detects if the observation leverages cognitive shortcuts.
/// Aggregates all detected bias categories.
///
/// # Examples
///
/// ```
/// use llmosafe::calculate_halo_signal;
/// let signal = calculate_halo_signal("The expert provided a professional opinion.");
/// assert!(signal > 0);
/// ```
pub fn calculate_halo_signal(text: &str) -> u16 {
    let breakdown = get_bias_breakdown(text);
    breakdown.values().sum()
}

/// Calculate the "Contextual Utility" (CPMI Proxy).
/// Measures how much the observation reduces uncertainty about the objective.
///
/// # Examples
///
/// ```
/// use llmosafe::calculate_utility;
/// let utility = calculate_utility("Rust is safe", "Rust safety");
/// assert!(utility > 0);
/// ```
pub fn calculate_utility(observation: &str, objective: &str) -> u16 {
    let obs_lower = observation.to_lowercase();
    let obj_lower = objective.to_lowercase();
    let obs_tokens: std::collections::BTreeSet<_> = obs_lower.split_whitespace().collect();
    let obj_tokens: std::collections::BTreeSet<_> = obj_lower.split_whitespace().collect();
    
    let intersection = obs_tokens.intersection(&obj_tokens).count();
    intersection.saturating_mul(100).min(u16::MAX as usize) as u16
}

/// Sift Perceptions (Think-and-Rank).
/// Converts a list of raw observations into a single Synapse spike.
/// Returns a high-entropy Synapse if no observations are provided.
///
/// # Examples
///
/// ```
/// use llmosafe::sift_perceptions;
/// let objective = "Safety";
/// let observations = &["Observation 1", "Observation 2"];
/// let synapse = sift_perceptions(observations, objective);
/// ```
pub fn sift_perceptions(observations: &[&str], objective: &str) -> Synapse {
    if observations.is_empty() {
        let mut synapse = Synapse::new();
        synapse.set_raw_entropy(0xFFFF); // Max instability
        synapse.set_raw_surprise(0);
        synapse.set_has_bias(false);
        synapse.set_anchor_hash(0);
        return synapse;
    }

    let mut scored_obs = Vec::new();
    let mut total_score: i32 = 0;

    for obs in observations {
        let utility = calculate_utility(obs, objective);
        let halo = calculate_halo_signal(obs);
        let score = (utility as i32) - (halo as i32);
        scored_obs.push((*obs, score));
        total_score += score;
    }

    // Rank by score
    scored_obs.sort_by(|a, b| b.1.cmp(&a.1));
    
    let top_anchor = scored_obs[0];
    let mean_score = total_score / (observations.len() as i32);

    // Quantize for BCP
    let entropy = 1000 - top_anchor.1;
    let surprise = (top_anchor.1 - mean_score).unsigned_abs() as u16;
    let has_bias = calculate_halo_signal(top_anchor.0) > 0;

    // Create the Synapse spike
    let mut synapse = Synapse::new();
    synapse.set_raw_entropy(entropy.clamp(0, 0xFFFF) as u16);
    synapse.set_raw_surprise(surprise);
    synapse.set_has_bias(has_bias);
    
    // Hash the anchor for integrity
    let anchor_hash = adler32::adler32(top_anchor.0.as_bytes());
    synapse.set_anchor_hash(anchor_hash & 0x3FFFFFFF);

    synapse
}

/// Adler32 simple implementation to avoid external dependencies in the module.
mod adler32 {
    pub fn adler32(data: &[u8]) -> u32 {
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_halo_signal() {
        assert_eq!(calculate_halo_signal("The lead expert is professional and official."), 600);
        assert_eq!(calculate_halo_signal("This is a random sentence without flags."), 0);
        assert_eq!(calculate_halo_signal("limited and exclusive special"), 600);
    }

    #[test]
    fn test_halo_signal_all_categories_detected() {
        let text = "expert popular limited now love sophisticated";
        let breakdown = get_bias_breakdown(text);
        assert_eq!(breakdown["authority_bias"], 200);
        assert_eq!(breakdown["social_proof"], 200);
        assert_eq!(breakdown["scarcity"], 200);
        assert_eq!(breakdown["urgency"], 200);
        assert_eq!(breakdown["emotional_appeal"], 200);
        assert_eq!(breakdown["expertise_signaling"], 200);
        assert_eq!(calculate_halo_signal(text), 1200);
    }

    #[test]
    fn test_sift_perceptions_empty_observations() {
        // No longer panics, returns max-entropy synapse
        let objective = "test";
        let observations: &[&str] = &[];
        
        let synapse = sift_perceptions(observations, objective);
        assert_eq!(synapse.raw_entropy(), 0xFFFF);
        assert_eq!(synapse.validate().unwrap_err(), crate::llmosafe_kernel::KernelError::CognitiveInstability);
    }

    #[test]
    fn test_sift_perceptions_single_observation() {
        let objective = "test";
        let observations = &["stable observation"];
        let synapse = sift_perceptions(observations, objective);
        assert!(synapse.validate().is_ok());
    }

    #[test]
    fn test_utility_calculation() {
        let objective = "Build a Rust safety library";
        let obs1 = "Rust safety is paramount";
        let obs2 = "C++ is also good";
        
        let u1 = calculate_utility(obs1, objective);
        let u2 = calculate_utility(obs2, objective);
        
        assert!(u1 > u2);
    }

    #[test]
    fn test_sifter_token_bomb() {
        let objective = "test";
        // Create an observation with 10,000 tokens
        let bomb = "token ".repeat(10000);
        let u = calculate_utility(&bomb, objective);
        // Should not overflow (saturating_mul handled it)
        let _ = u;
    }

    #[test]
    fn test_halo_signal_keyword_density() {
        // Many keywords in one string
        let text = "expert professional authorized reliable trusted specialist veteran master guru authority";
        let signal = calculate_halo_signal(text);
        assert!(signal >= 2000);
    }

    #[test]
    fn test_halo_signal_metamorphic_monotonicity() {
        let text1 = "This is a normal observation.";
        let text2 = "This is an expert observation.";
        let text3 = "This is an expert and professional observation.";
        
        let s1 = calculate_halo_signal(text1);
        let s2 = calculate_halo_signal(text2);
        let s3 = calculate_halo_signal(text3);
        
        assert!(s1 <= s2);
        assert!(s2 <= s3);
        assert!(s3 > s1);
    }

    #[test]
    fn test_utility_metamorphic_shuffle() {
        let objective = "Safety Critical AI";
        let obs1 = "Formal verification ensures deterministic execution.";
        let obs2 = "execution deterministic ensures verification Formal."; // Shuffled
        
        let u1 = calculate_utility(obs1, objective);
        let u2 = calculate_utility(obs2, objective);
        
        assert_eq!(u1, u2);
    }

    #[test]
    fn test_sift_quantization_differential() {
        let objective = "Safety";
        let observations = &["Safety is paramount"];
        
        // Manual calculation:
        // utility = 1 token match * 100 = 100
        // halo = 0
        // score = 100
        // entropy = 1000 - 100 = 900
        
        let synapse = sift_perceptions(observations, objective);
        assert_eq!(synapse.raw_entropy(), 900);
    }

    #[test]
    fn test_sift_perceptions_logic() {
        let objective = "Identify the best coding language for safety";
        let observations = &[
            "Rust is the most secure language due to its ownership model",
            "Python is very popular and easy to learn",
            "C is exclusive because it is limited but unsafe"
        ];

        let synapse = sift_perceptions(observations, objective);

        // Rust should be the top anchor because it has more utility and less halo than C
        // 'C' has 'exclusive' and 'limited' keywords, giving it halo.
        assert!(synapse.validate().is_ok());
        assert!(synapse.anchor_hash() != 0);
    }
}
