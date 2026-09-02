// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {
    AggregatorV3Interface,
    AggregatorInterface,
    IAggregatorBounds
} from "./interfaces/AggregatorV3Interface.sol";
import {CrossfootAttestations} from "./CrossfootAttestations.sol";

/// @title CrossfootGuard
/// @notice An AggregatorV3-compatible facade over any feed that enforces a consumer-owned
/// posting policy: a per-update deviation bound, an absolute delta cap, a velocity limit
/// over a window, a minimum interval, a freshness limit, min and max answers, the source's
/// own floor and ceiling, and an optional "requires attributed path" rule fed by
/// CrossfootAttestations. Specification: docs/specs/10-guard-wrapper.md.
///
/// The guard never changes a value. It either serves the source's latest round, because
/// that round passed the policy against the last accepted round, or it refuses: per
/// consumer, by reverting or by serving the last accepted answer with stale semantics
/// (`answeredInRound` below `roundId`, `updatedAt` of the accepted round). A rejected
/// round recorded by `sync()` halts the guard until the owner resumes it; a guardian can
/// pause it at any time; policy and role changes wait for the timelock.
///
/// The policy is the consumer's rule, not the feed's. A rejection says that a post moved
/// further, faster or through a different path than this consumer accepts. It does not
/// say that the posted value was wrong.
contract CrossfootGuard is AggregatorV3Interface, AggregatorInterface {
    // ------------------------------------------------------------------ types

    /// @notice Why a source round is not served. `None` means the round is served.
    enum Reason {
        None,
        NonPositive,
        OutOfRange,
        AtSourceBound,
        Deviation,
        AbsoluteDelta,
        Velocity,
        Interval,
        AttestationMissing,
        AttestationStale,
        AttestationReview,
        Halted,
        Paused
    }

    /// @notice What a consumer receives when the guard refuses.
    enum Mode {
        Default,
        Revert,
        LastAccepted
    }

    /// @notice 0: no attestation rule. 1: a REVIEW record blocks and a new round needs an
    /// ALLOW record not older than `maxAttestationAge`. 2: as 1, and the ALLOW record must
    /// cover the round (`coveredRoundId >= roundId`).
    uint8 public constant ATTEST_NONE = 0;
    uint8 public constant ATTEST_REVIEW_BLOCKS = 1;
    uint8 public constant ATTEST_PER_ROUND = 2;

    /// @notice Deviation and velocity are percentages at the Midas scale: 1e8 is one
    /// percent, so 100 percent is 1e10 and the mRE7 bound of 0.36 percent is 36,000,000.
    /// The formula is the one Crossfoot replays: |value - last| * 1e10 / |last|,
    /// truncating.
    uint256 public constant PERCENT = 1e8;

    /// @notice Decision codes of CrossfootAttestations.
    uint8 public constant DECISION_ALLOW = 1;
    uint8 public constant DECISION_REVIEW = 2;

    struct Policy {
        uint64 maxDeviation; // per accepted round, percent at 1e8; 0 = off
        uint64 maxVelocity; // against the window anchor, percent at 1e8; 0 = off
        uint32 velocityWindow; // seconds; the anchor is the first accepted round of a window
        uint32 maxStaleness; // seconds since the source's updatedAt; 0 = off
        uint32 minInterval; // seconds between accepted rounds; 0 = off
        uint32 maxAttestationAge; // seconds; attestation modes 1 and 2
        uint8 attestationMode; // ATTEST_NONE, ATTEST_REVIEW_BLOCKS, ATTEST_PER_ROUND
        bool haltOnReject; // a rejected round recorded by sync() halts the guard
        bool revertByDefault; // Mode.Default resolves to Revert (true) or LastAccepted
        int128 minAnswer; // both zero = off
        int128 maxAnswer;
        uint128 maxAbsoluteDelta; // in answer units; 0 = off
        address boundsSource; // aggregator exposing minAnswer() and maxAnswer(); 0 = off
    }

    struct Accepted {
        int256 answer;
        uint80 roundId;
        uint64 updatedAt; // the source's timestamp for the round
        uint64 acceptedAt; // block timestamp of the acceptance
    }

    struct Anchor {
        int256 answer;
        uint64 openedAt; // updatedAt of the first accepted round of the window
    }

    struct Status {
        bool halted;
        bool paused;
        Reason haltReason;
        uint80 haltRoundId;
        uint64 haltedAt;
    }

    struct Evaluation {
        Reason reason;
        bool stale;
        uint80 roundId;
        int256 answer;
        uint256 startedAt;
        uint256 updatedAt;
        uint256 measured; // in the failing check's own units
        uint256 limit;
    }

    struct PendingPolicy {
        Policy policy;
        uint64 readyAt;
        bool exists;
    }

    struct PendingRoles {
        address owner;
        address guardian;
        address attester;
        uint64 readyAt;
        bool exists;
    }

    // ------------------------------------------------------------------ state

    AggregatorV3Interface public immutable source;
    CrossfootAttestations public immutable attestations;
    uint64 public immutable timelockDelay;
    uint8 private immutable _decimals;

    address public owner;
    address public guardian;
    address public attester;

    Policy internal policy;
    Accepted public lastAccepted;
    Anchor public anchor;
    Status public status;

    PendingPolicy internal pendingPolicy;
    PendingRoles public pendingRoles;

    mapping(address consumer => Mode) public consumerMode;

    // ----------------------------------------------------------------- events

    event RoundAccepted(uint80 indexed roundId, int256 answer, uint64 updatedAt);
    event RoundRejected(
        uint80 indexed roundId, int256 answer, Reason reason, uint256 measured, uint256 limit
    );
    event Halted(Reason reason, uint80 roundId);
    event Paused(address indexed by);
    event Resumed(address indexed by, bool rebased, uint80 roundId, int256 answer);
    event ConsumerModeSet(address indexed consumer, Mode mode);
    event PolicyProposed(uint64 readyAt);
    event PolicyApplied();
    event RolesProposed(address owner, address guardian, address attester, uint64 readyAt);
    event RolesApplied(address owner, address guardian, address attester);
    event ProposalsCancelled();

    // ----------------------------------------------------------------- errors

    error GuardRejected(Reason reason, uint256 measured, uint256 limit);
    error GuardStale(uint80 roundId, uint256 updatedAt, uint256 limit);
    error NotOwner();
    error NotGuardian();
    error NothingPending();
    error TimelockActive(uint64 readyAt);
    error HistoricalRoundsNotGuarded();
    error BadPolicy();
    error BaselineNonPositive();

    // ------------------------------------------------------------ construction

    /// @param source_ The feed to wrap; its decimals and description are passed through.
    /// @param attestations_ The registry on this chain; may be zero when
    /// `policy_.attestationMode` is ATTEST_NONE.
    /// @param policy_ The initial policy. Later changes go through the timelock.
    /// @param owner_ Resumes, applies proposals, sets consumer modes for others.
    /// @param guardian_ May pause. Intended for the guardian agent of spec 11.
    /// @param attester_ The key whose CrossfootAttestations records the guard reads.
    /// @param timelockDelay_ Seconds between a proposal and its application.
    ///
    /// The constructor accepts the source's current round as the baseline without checks:
    /// the deployer vouches for it, and the event log records it as `RoundAccepted`.
    constructor(
        AggregatorV3Interface source_,
        CrossfootAttestations attestations_,
        Policy memory policy_,
        address owner_,
        address guardian_,
        address attester_,
        uint64 timelockDelay_
    ) {
        _checkPolicy(policy_, address(attestations_));
        source = source_;
        attestations = attestations_;
        timelockDelay = timelockDelay_;
        _decimals = source_.decimals();
        owner = owner_;
        guardian = guardian_;
        attester = attester_;
        policy = policy_;

        (uint80 roundId, int256 answer,, uint256 updatedAt,) = source_.latestRoundData();
        if (answer <= 0) revert BaselineNonPositive();
        _accept(roundId, answer, uint64(updatedAt));
    }

    // -------------------------------------------------------------- read path

    /// @notice The policy in force. A struct getter, because the auto-generated one would
    /// return thirteen values.
    function getPolicy() external view returns (Policy memory) {
        return policy;
    }

    function getPendingPolicy() external view returns (PendingPolicy memory) {
        return pendingPolicy;
    }

    function decimals() external view override returns (uint8) {
        return _decimals;
    }

    function description() external view override returns (string memory) {
        return string.concat("CrossfootGuard: ", source.description());
    }

    function version() external pure override returns (uint256) {
        return 1;
    }

    /// @notice Historical rounds were not necessarily accepted by this policy; the guard
    /// does not serve them. Every value the guard returns passed the policy.
    function getRoundData(uint80)
        external
        pure
        override
        returns (uint80, int256, uint256, uint256, uint80)
    {
        revert HistoricalRoundsNotGuarded();
    }

    /// @notice The guarded read. Serves the source's latest round when it passes the
    /// policy against the last accepted round; otherwise reverts or serves the last
    /// accepted answer with stale semantics, per the caller's mode.
    function latestRoundData()
        public
        view
        override
        returns (
            uint80 roundId,
            int256 answer,
            uint256 startedAt,
            uint256 updatedAt,
            uint80 answeredInRound
        )
    {
        Evaluation memory e = evaluate();
        if (e.reason == Reason.None && !e.stale) {
            return (e.roundId, e.answer, e.startedAt, e.updatedAt, e.roundId);
        }
        Mode mode = _modeOf(msg.sender);
        if (mode == Mode.Revert) {
            if (e.reason != Reason.None) revert GuardRejected(e.reason, e.measured, e.limit);
            revert GuardStale(e.roundId, e.updatedAt, policy.maxStaleness);
        }
        Accepted memory last = lastAccepted;
        // roundId is the source's round so a consumer sees that a newer round exists;
        // answeredInRound is the accepted round, below roundId, which is the Chainlink
        // convention for "this answer is not from the latest round"; updatedAt is the
        // accepted round's own timestamp, so a consumer's staleness check trips.
        uint80 servedRound = e.roundId > last.roundId ? e.roundId : last.roundId;
        return (servedRound, last.answer, last.updatedAt, last.updatedAt, last.roundId);
    }

    function latestAnswer() external view override returns (int256) {
        (, int256 answer,,,) = latestRoundData();
        return answer;
    }

    function latestTimestamp() external view override returns (uint256) {
        (,,, uint256 updatedAt,) = latestRoundData();
        return updatedAt;
    }

    function latestRound() external view override returns (uint256) {
        (uint80 roundId,,,,) = latestRoundData();
        return roundId;
    }

    /// @notice The full evaluation of the source's latest round against the policy, for
    /// consumers, keepers and the guardian agent. `reason` is `None` when the round is
    /// served; `stale` is reported separately because a stale round is refused on the
    /// read path but is not a rejection that halts the guard.
    function evaluate() public view returns (Evaluation memory e) {
        (e.roundId, e.answer, e.startedAt, e.updatedAt,) = source.latestRoundData();
        Policy memory p = policy;
        e.stale = p.maxStaleness != 0 && block.timestamp > e.updatedAt + p.maxStaleness;

        Status memory st = status;
        if (st.paused) {
            e.reason = Reason.Paused;
            return e;
        }
        if (st.halted) {
            e.reason = Reason.Halted;
            return e;
        }

        Accepted memory last = lastAccepted;
        // A feed that rewrites the answer or the timestamp under the same round id is
        // treated as a new round, so the same-round shortcut cannot be used to slip a
        // value past the checks.
        bool newRound =
            e.roundId != last.roundId || e.answer != last.answer || e.updatedAt != last.updatedAt;

        if (p.attestationMode != ATTEST_NONE) {
            (e.reason, e.measured, e.limit) = _attestationCheck(p, newRound, e.roundId);
            if (e.reason != Reason.None) return e;
        }
        if (!newRound) return e;

        (e.reason, e.measured, e.limit) = _check(p, last, e.answer, e.updatedAt);
    }

    /// @dev An attestation of REVIEW blocks even the accepted round: the attester found a
    /// post on this feed that did not go through the feed's own rule. A missing or stale
    /// record only blocks a new round, so the guard fails open on attester liveness for
    /// state it already accepted and closed for state it has not.
    function _attestationCheck(Policy memory p, bool newRound, uint80 roundId)
        internal
        view
        returns (Reason, uint256, uint256)
    {
        (uint8 decision, uint80 covered, uint64 attestedAt) =
            attestations.decisionFor(attester, address(source));
        if (decision == DECISION_REVIEW) return (Reason.AttestationReview, covered, attestedAt);
        if (!newRound) return (Reason.None, 0, 0);
        if (decision == 0) return (Reason.AttestationMissing, 0, 0);
        if (block.timestamp > attestedAt + p.maxAttestationAge) {
            return (Reason.AttestationStale, block.timestamp - attestedAt, p.maxAttestationAge);
        }
        if (p.attestationMode == ATTEST_PER_ROUND && covered < roundId) {
            return (Reason.AttestationMissing, covered, roundId);
        }
        return (Reason.None, 0, 0);
    }

    // ------------------------------------------------------------- write path

    /// @notice Records the evaluation: accepts a passing new round as the reference for
    /// the next checks, or records the rejection and halts the guard when the policy says
    /// so. Anyone may call; a keeper, the consumer's own accrual path, or the guardian
    /// agent. Calling it is what turns "the read path refuses this value" into "the
    /// market is frozen pending review".
    function sync() external returns (Reason reason) {
        Evaluation memory e = evaluate();
        reason = e.reason;
        if (reason == Reason.Paused || reason == Reason.Halted) return reason;
        if (reason == Reason.None) {
            Accepted memory last = lastAccepted;
            if (
                e.roundId != last.roundId || e.answer != last.answer
                    || e.updatedAt != last.updatedAt
            ) {
                _accept(e.roundId, e.answer, uint64(e.updatedAt));
            }
            return reason;
        }
        emit RoundRejected(e.roundId, e.answer, reason, e.measured, e.limit);
        if (policy.haltOnReject) {
            status = Status({
                halted: true,
                paused: status.paused,
                haltReason: reason,
                haltRoundId: e.roundId,
                haltedAt: uint64(block.timestamp)
            });
            emit Halted(reason, e.roundId);
        }
    }

    /// @notice A consumer chooses what it receives on refusal. A consumer that never
    /// calls this gets the policy default.
    function setConsumerMode(Mode mode) external {
        consumerMode[msg.sender] = mode;
        emit ConsumerModeSet(msg.sender, mode);
    }

    function setConsumerModeFor(address consumer, Mode mode) external onlyOwner {
        consumerMode[consumer] = mode;
        emit ConsumerModeSet(consumer, mode);
    }

    // -------------------------------------------------------------- guardian

    /// @notice Immediate. The guardian's only power.
    function pause() external {
        if (msg.sender != guardian && msg.sender != owner) revert NotGuardian();
        status.paused = true;
        emit Paused(msg.sender);
    }

    /// @notice Clears a pause and a halt. With `rebase` the source's current round is
    /// accepted as the new reference without checks: the owner has reviewed it. Without
    /// `rebase` the next round is measured against the round accepted before the halt.
    function resume(bool rebase) external onlyOwner {
        delete status;
        uint80 roundId;
        int256 answer;
        if (rebase) {
            uint256 updatedAt;
            (roundId, answer,, updatedAt,) = source.latestRoundData();
            if (answer <= 0) revert BaselineNonPositive();
            _accept(roundId, answer, uint64(updatedAt));
        } else {
            roundId = lastAccepted.roundId;
            answer = lastAccepted.answer;
        }
        emit Resumed(msg.sender, rebase, roundId, answer);
    }

    // -------------------------------------------------------------- timelock

    function proposePolicy(Policy calldata next) external onlyOwner {
        _checkPolicy(next, address(attestations));
        uint64 readyAt = uint64(block.timestamp) + timelockDelay;
        pendingPolicy = PendingPolicy({policy: next, readyAt: readyAt, exists: true});
        emit PolicyProposed(readyAt);
    }

    function applyPolicy() external onlyOwner {
        PendingPolicy memory pp = pendingPolicy;
        if (!pp.exists) revert NothingPending();
        if (block.timestamp < pp.readyAt) revert TimelockActive(pp.readyAt);
        policy = pp.policy;
        delete pendingPolicy;
        emit PolicyApplied();
    }

    function proposeRoles(address owner_, address guardian_, address attester_) external onlyOwner {
        uint64 readyAt = uint64(block.timestamp) + timelockDelay;
        pendingRoles = PendingRoles({
            owner: owner_, guardian: guardian_, attester: attester_, readyAt: readyAt, exists: true
        });
        emit RolesProposed(owner_, guardian_, attester_, readyAt);
    }

    function applyRoles() external onlyOwner {
        PendingRoles memory pr = pendingRoles;
        if (!pr.exists) revert NothingPending();
        if (block.timestamp < pr.readyAt) revert TimelockActive(pr.readyAt);
        owner = pr.owner;
        guardian = pr.guardian;
        attester = pr.attester;
        delete pendingRoles;
        emit RolesApplied(pr.owner, pr.guardian, pr.attester);
    }

    function cancelProposals() external onlyOwner {
        delete pendingPolicy;
        delete pendingRoles;
        emit ProposalsCancelled();
    }

    // ------------------------------------------------------------- internals

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    function _modeOf(address consumer) internal view returns (Mode) {
        Mode m = consumerMode[consumer];
        if (m != Mode.Default) return m;
        return policy.revertByDefault ? Mode.Revert : Mode.LastAccepted;
    }

    function _accept(uint80 roundId, int256 answer, uint64 updatedAt) internal {
        Anchor memory a = anchor;
        if (a.openedAt == 0 || updatedAt > a.openedAt + policy.velocityWindow) {
            anchor = Anchor({answer: answer, openedAt: updatedAt});
        }
        lastAccepted = Accepted({
            answer: answer,
            roundId: roundId,
            updatedAt: updatedAt,
            acceptedAt: uint64(block.timestamp)
        });
        emit RoundAccepted(roundId, answer, updatedAt);
    }

    /// @dev The checks a new round must pass, in the order they are reported. The first
    /// failing check is the reason; `measured` and `limit` are in the check's own units.
    function _check(Policy memory p, Accepted memory last, int256 answer, uint256 updatedAt)
        internal
        view
        returns (Reason, uint256 measured, uint256 limit)
    {
        if (answer <= 0) return (Reason.NonPositive, 0, 0);
        if (p.minAnswer != 0 || p.maxAnswer != 0) {
            if (answer < p.minAnswer || answer > p.maxAnswer) {
                return (Reason.OutOfRange, uint256(answer), uint256(int256(p.maxAnswer)));
            }
        }
        if (p.boundsSource != address(0)) {
            int256 floor = IAggregatorBounds(p.boundsSource).minAnswer();
            int256 ceiling = IAggregatorBounds(p.boundsSource).maxAnswer();
            if (answer <= floor || answer >= ceiling) {
                return
                    (
                        Reason.AtSourceBound,
                        uint256(answer),
                        uint256(answer <= floor ? floor : ceiling)
                    );
            }
        }
        if (p.minInterval != 0 && updatedAt < uint256(last.updatedAt) + p.minInterval) {
            return (Reason.Interval, updatedAt - last.updatedAt, p.minInterval);
        }
        if (p.maxDeviation != 0) {
            uint256 d = deviation(last.answer, answer);
            if (d > p.maxDeviation) return (Reason.Deviation, d, p.maxDeviation);
        }
        if (p.maxAbsoluteDelta != 0) {
            uint256 delta = _absDiff(last.answer, answer);
            if (delta > p.maxAbsoluteDelta) {
                return (Reason.AbsoluteDelta, delta, p.maxAbsoluteDelta);
            }
        }
        if (p.maxVelocity != 0) {
            Anchor memory a = anchor;
            int256 windowBase = updatedAt > a.openedAt + p.velocityWindow ? last.answer : a.answer;
            uint256 v = deviation(windowBase, answer);
            if (v > p.maxVelocity) return (Reason.Velocity, v, p.maxVelocity);
        }
        return (Reason.None, 0, 0);
    }

    /// @notice |value - last| * 1e10 / |last|, the Midas formula at percent times 1e8.
    function deviation(int256 last, int256 value) public pure returns (uint256) {
        uint256 base = _abs(last);
        if (base == 0) return type(uint256).max;
        return _absDiff(last, value) * (100 * PERCENT) / base;
    }

    function _absDiff(int256 a, int256 b) internal pure returns (uint256) {
        return a > b ? uint256(a - b) : uint256(b - a);
    }

    function _abs(int256 x) internal pure returns (uint256) {
        return x < 0 ? uint256(-x) : uint256(x);
    }

    function _checkPolicy(Policy memory p, address registry) internal pure {
        if (p.attestationMode > ATTEST_PER_ROUND) revert BadPolicy();
        if (p.attestationMode != ATTEST_NONE && registry == address(0)) revert BadPolicy();
        if (p.maxVelocity != 0 && p.velocityWindow == 0) revert BadPolicy();
        if ((p.minAnswer != 0 || p.maxAnswer != 0) && p.minAnswer > p.maxAnswer) {
            revert BadPolicy();
        }
    }
}
