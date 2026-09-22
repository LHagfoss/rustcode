//! Compatibility facade for loop detection now owned by a dependency-light
//! workspace crate.

pub(crate) use rustcode_loop_detect::*;

/// Bounded semantic result used by the optional repetition advisory.  The
/// local detector remains authoritative; this classification can only decide
/// whether one already-bounded read-only recovery credit is consumable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryAdvisory {
    NovelEvidence,
    ConfirmatoryEvidence,
    NoNewInformation,
    Unknown,
}

pub(crate) fn recovery_advisory(
    decision: &crate::laya::AdvisoryDecision,
    _min_confidence: f32,
) -> RecoveryAdvisory {
    match decision.label.as_str() {
        "novel_evidence" => RecoveryAdvisory::NovelEvidence,
        "confirmatory_evidence" => RecoveryAdvisory::ConfirmatoryEvidence,
        "no_new_information" => RecoveryAdvisory::NoNewInformation,
        _ => RecoveryAdvisory::Unknown,
    }
}

pub(crate) fn laya_recovery_credit_available(
    mode: crate::laya::LayaMode,
    decision: &crate::laya::AdvisoryDecision,
    min_confidence: f32,
    used: usize,
    configured_credits: usize,
    eligible_read_only_batch: bool,
) -> bool {
    mode == crate::laya::LayaMode::Relaxed
        && eligible_read_only_batch
        && configured_credits > 0
        && used == 0
        && decision.confidence.is_finite()
        && decision.confidence >= min_confidence
        && decision.effects.iter().all(|effect| effect == "read_only")
        && !decision.effects.is_empty()
        && matches!(
            recovery_advisory(decision, min_confidence),
            RecoveryAdvisory::NovelEvidence | RecoveryAdvisory::ConfirmatoryEvidence
        )
}
