// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title CrossfootAttestations
/// @notice A decision registry: per attester and per feed, the latest Crossfoot consumer
/// decision (ALLOW or REVIEW) together with the provenance fields the off-chain decision
/// record already carries. The shape is the one of the deferred Arc hook
/// (docs/specs/06-arc-hook.md R1 and R2) with one added field, `coveredRoundId`, the
/// latest feed round the decision attributed. Deployed on the chain where the lender
/// lives, so CrossfootGuard can read it in the same call.
///
/// The registry interprets nothing. Anyone may attest; records are keyed by the attester,
/// so no caller overwrites another's. Which attester a guard trusts is the guard's policy.
contract CrossfootAttestations {
    uint8 public constant ALLOW = 1;
    uint8 public constant REVIEW = 2;

    struct Record {
        uint8 decision;
        uint80 coveredRoundId;
        uint64 sourceBlock;
        uint64 attestedAt;
        bytes32 recordHash;
        bytes32 deploymentDigest;
        bytes32 bundleRoot;
    }

    mapping(address attester => mapping(address feed => Record)) private _latest;

    event Attested(
        address indexed attester,
        address indexed feed,
        uint8 decision,
        uint80 coveredRoundId,
        bytes32 recordHash,
        bytes32 deploymentDigest,
        uint64 sourceBlock,
        bytes32 bundleRoot
    );

    error BadDecision();

    /// @param feed The audited feed (the source a guard wraps), not the guard.
    /// @param decision 1 for ALLOW, 2 for REVIEW.
    /// @param coveredRoundId The latest round of `feed` the decision record attributed.
    /// @param recordHash `record_sha256` of the decision record (05 R13).
    /// @param deploymentDigest `provenance.subgraph.deployment_digest`.
    /// @param sourceBlock `provenance.subgraph.block.number`.
    /// @param bundleRoot `evidence.crossfoot.bundle_root`, zero when the record has none.
    function attest(
        address feed,
        uint8 decision,
        uint80 coveredRoundId,
        bytes32 recordHash,
        bytes32 deploymentDigest,
        uint64 sourceBlock,
        bytes32 bundleRoot
    ) external {
        if (decision != ALLOW && decision != REVIEW) revert BadDecision();
        _latest[msg.sender][feed] = Record({
            decision: decision,
            coveredRoundId: coveredRoundId,
            sourceBlock: sourceBlock,
            attestedAt: uint64(block.timestamp),
            recordHash: recordHash,
            deploymentDigest: deploymentDigest,
            bundleRoot: bundleRoot
        });
        emit Attested(
            msg.sender,
            feed,
            decision,
            coveredRoundId,
            recordHash,
            deploymentDigest,
            sourceBlock,
            bundleRoot
        );
    }

    function latest(address attester, address feed) external view returns (Record memory) {
        return _latest[attester][feed];
    }

    /// @notice The one-slot read a guard needs on its hot path.
    function decisionFor(address attester, address feed)
        external
        view
        returns (uint8 decision, uint80 coveredRoundId, uint64 attestedAt)
    {
        Record storage r = _latest[attester][feed];
        return (r.decision, r.coveredRoundId, r.attestedAt);
    }
}
