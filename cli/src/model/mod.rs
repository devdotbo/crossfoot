//! The model layer.
//!
//! Two independent implementations of the same accrual, plus the comparison
//! against chain state:
//!
//! - `replay`: the deployed integer state machine, transcribed from the
//!   Solidity, floors where the contract floors.
//! - `actus`: the vendored ACTUS engine, driven by a harness that owns the
//!   endogenous schedule and every integer quantisation.
//!
//! Disagreement between the two is a build failure. Disagreement between the
//! two agreeing models and the chain is a finding.

pub mod actus;
pub mod clock;
pub mod decision;
pub mod mtbill;
pub mod replay;
pub mod verdict;
pub mod wide;
