//! Brain-inspired relevance scoring based on the Ebbinghaus forgetting curve
//! with reinforcement from spaced repetition research.
//!
//! The model combines:
//! - Exponential decay (memories fade without rehearsal)
//! - Logarithmic strengthening from repeated access (spacing effect)
//! - Importance-based floor (amygdala-tagged memories never fully fade)
//! - Adaptive decay rates (important memories decay slower)

/// Compute the relevance score for a fragment.
///
/// Formula: `relevance = importance * strength * exp(-decay_rate * days_since) + importance * 0.3`
///
/// Where `strength = min(1.0, 0.3 + 0.15 * ln(1 + access_count))`
pub fn compute_relevance(
    importance: f32,
    access_count: u32,
    decay_rate: f32,
    last_reinforced: i64,
    now: i64,
) -> f32 {
    let days_since = ((now - last_reinforced) as f32 / 86400.0).max(0.0);

    // Logarithmic strength from access frequency (spacing effect)
    // Each retrieval strengthens less than the last, but cumulatively builds durability
    let strength = (0.3 + 0.15 * (1.0 + access_count as f32).ln()).min(1.0);

    // Ebbinghaus forgetting curve: exponential decay
    let decay = (-decay_rate * days_since).exp();

    // Importance floor: high-salience memories never fully decay
    let floor = importance * 0.3;

    (importance * strength * decay + floor).clamp(0.0, 1.0)
}

/// Compute the appropriate decay rate for a given importance level.
///
/// - importance 1.0 → lambda 0.01 (half-life ~70 days)
/// - importance 0.5 → lambda 0.035 (half-life ~20 days)
/// - importance 0.1 → lambda 0.07 (half-life ~10 days)
pub fn decay_rate_for_importance(importance: f32) -> f32 {
    0.07 - 0.06 * importance.clamp(0.0, 1.0)
}

/// The blending weight for semantic similarity vs relevance in query scoring.
/// score = SEMANTIC_WEIGHT * cosine_sim + (1 - SEMANTIC_WEIGHT) * relevance
pub const SEMANTIC_WEIGHT: f32 = 0.7;

/// Minimum relevance score for a fragment to appear in query results.
/// Below this threshold, fragments are effectively "forgotten" — they exist
/// in the database but are invisible to queries.
pub const MIN_RELEVANCE_THRESHOLD: f32 = 0.05;

/// Boost factor for spreading activation to neighbors on access.
pub const ACTIVATION_SPREAD_FACTOR: f32 = 0.1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fresh_fragment_has_high_relevance() {
        let now = 1000000;
        let r = compute_relevance(0.5, 0, 0.035, now, now);
        // Fresh fragment: strength = 0.3, decay = 1.0, floor = 0.15
        // relevance = 0.5 * 0.3 * 1.0 + 0.15 = 0.30
        assert!(r > 0.25 && r < 0.35, "Got {}", r);
    }

    #[test]
    fn test_accessed_fragment_strengthens() {
        let now = 1000000;
        let r0 = compute_relevance(0.5, 0, 0.035, now, now);
        let r10 = compute_relevance(0.5, 10, 0.035, now, now);
        let r100 = compute_relevance(0.5, 100, 0.035, now, now);
        assert!(r10 > r0, "10 accesses should be stronger than 0");
        assert!(r100 > r10, "100 accesses should be stronger than 10");
    }

    #[test]
    fn test_old_fragment_decays() {
        let now = 1000000;
        let day = 86400;
        let r_fresh = compute_relevance(0.5, 0, 0.035, now, now);
        let r_20d = compute_relevance(0.5, 0, 0.035, now - 20 * day, now);
        let r_60d = compute_relevance(0.5, 0, 0.035, now - 60 * day, now);
        assert!(r_20d < r_fresh, "20-day old should be weaker");
        assert!(r_60d < r_20d, "60-day old should be weaker still");
    }

    #[test]
    fn test_important_fragment_has_floor() {
        let now = 1000000;
        let day = 86400;
        // Very old fragment with high importance
        let r = compute_relevance(1.0, 0, 0.01, now - 365 * day, now);
        // Floor = 1.0 * 0.3 = 0.3, so even after a year it should be >= 0.3
        assert!(
            r >= 0.29,
            "High importance should have floor ~0.3, got {}",
            r
        );
    }

    #[test]
    fn test_unimportant_fragment_fades_to_near_zero() {
        let now = 1000000;
        let day = 86400;
        let r = compute_relevance(0.1, 0, 0.07, now - 90 * day, now);
        // Floor = 0.1 * 0.3 = 0.03, should be near floor
        assert!(
            r < 0.06,
            "Low importance old fragment should nearly vanish, got {}",
            r
        );
    }

    #[test]
    fn test_decay_rate_for_importance() {
        let high = decay_rate_for_importance(1.0);
        let mid = decay_rate_for_importance(0.5);
        let low = decay_rate_for_importance(0.1);
        assert!(high < mid, "High importance should decay slower");
        assert!(mid < low, "Mid importance should decay slower than low");
        assert!((high - 0.01).abs() < 0.001);
        assert!((low - 0.064).abs() < 0.001);
    }

    #[test]
    fn test_relevance_clamped() {
        let now = 1000000;
        let r = compute_relevance(1.0, 1000, 0.001, now, now);
        assert!(r <= 1.0, "Should be clamped to 1.0, got {}", r);
        assert!(r >= 0.0);
    }
}
