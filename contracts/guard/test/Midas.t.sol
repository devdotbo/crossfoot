// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Fixture} from "./Fixture.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {CrossfootAttestations} from "../src/CrossfootAttestations.sol";
import {OwnerPostedFeed} from "./mocks/OwnerPostedFeed.sol";
import {Consumer} from "./mocks/Consumer.sol";

/// @notice Replays mRE7.customFeed rounds 28 to 36 (answers and timestamps from the
/// Crossfoot fixture bundle at block 25,884,405, timelines/mre7-customfeed.json; spec 02
/// R19) through a guard whose bound is the feed's own bound in force at block 25,037,958:
/// 0.36 percent. Rounds 29 to 35 went through setRoundDataSafe and pass. Round 36 went
/// through setRoundData, the documented high-deviation path without the on-chain check,
/// at 2.22466613 percent: the guard rejects it with the same number Crossfoot reports.
/// The guard does not know the path; the attestation test below is where the path
/// enters.
contract MidasRound36Test is Fixture {
    address constant MRE7 = 0x0a2a51f2f206447dE3E3a80FCf92240244722395;
    bytes32 constant ROUND_36_TX =
        0x7579ba75c3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733;

    OwnerPostedFeed feed;
    CrossfootGuard guard;
    Consumer lender;

    uint256[9] rounds = [28, 29, 30, 31, 32, 33, 34, 35, 36];
    int256[9] answers = [
        int256(108979382),
        109134340,
        109139138,
        109139897,
        109139909,
        109182355,
        109182701,
        108859885,
        106438116
    ];
    uint256[9] times = [
        1770140579,
        1770750827,
        1771962935,
        1772216219,
        1773155159,
        1773955907,
        1774636955,
        1776450335,
        1778094239
    ];

    function _policy(uint64 bound) internal pure returns (CrossfootGuard.Policy memory p) {
        p = _emptyPolicy();
        p.maxDeviation = bound;
    }

    function setUp() public {
        feed = new OwnerPostedFeed(8, "mRE7/USD");
        _post(feed, rounds[0], times[0], answers[0]);
        guard = _deploy(feed, _policy(36_000_000));
        lender = new Consumer(guard);
    }

    function _replayGuardedRounds() internal {
        for (uint256 i = 1; i < 8; i++) {
            _post(feed, rounds[i], times[i], answers[i]);
            assertEq(
                uint256(guard.sync()),
                uint256(CrossfootGuard.Reason.None),
                string.concat("round ", _u(rounds[i]))
            );
        }
        assertEq(_lastAnswer(guard), 108859885, "round 35 is the reference");
    }

    function test_rounds_29_to_35_pass_the_bound_in_force() public {
        _replayGuardedRounds();
    }

    function test_round_36_is_rejected_at_the_deviation_crossfoot_reports() public {
        _replayGuardedRounds();
        _post(feed, 36, times[8], answers[8]);
        (CrossfootGuard.Reason r, uint256 measured, uint256 limit) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Deviation), "reason");
        assertEq(measured, 222466613, "deviation_in_force of the spec 02 R19 row");
        assertEq(limit, 36_000_000, "bound_in_force");
        assertEq(guard.deviation(108859885, 106438116), 222466613, "the Midas formula");
        assertEq(uint256(guard.sync()), uint256(CrossfootGuard.Reason.Deviation), "sync");
        assertTrue(_halted(guard), "frozen");
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector, CrossfootGuard.Reason.Halted, 0, 0
            )
        );
        lender.read();
    }

    function test_round_36_is_rejected_under_the_earlier_two_percent_bound_as_well() public {
        guard = _deploy(feed, _policy(200_000_000));
        _replayGuardedRounds();
        _post(feed, 36, times[8], answers[8]);
        (CrossfootGuard.Reason r, uint256 measured,) = _reason(guard);
        assertEq(
            uint256(r), uint256(CrossfootGuard.Reason.Deviation), "2.22 percent is over 2.0 percent"
        );
        assertEq(measured, 222466613, "measured");
    }

    /// @notice The attributed-path policy: a guard with no bound of its own, fed by the
    /// Crossfoot decision for the feed. The attester records the consumer decision of
    /// spec 05 R14 (REVIEW, ADMIN_GUARD_BYPASSED, round 36) and the guard refuses from
    /// that moment, whatever the value.
    function test_a_review_attestation_blocks_the_feed() public {
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.attestationMode = 1;
        p.maxAttestationAge = 7 days;
        guard = _deploy(feed, p);
        lender = new Consumer(guard);

        // The consumer agent runs on its schedule and attests ALLOW while every round
        // takes the checked path; here once per round, before the guard sees it.
        for (uint256 i = 1; i < 8; i++) {
            _post(feed, rounds[i], times[i], answers[i]);
            vm.prank(ATTESTER);
            registry.attest(
                address(feed), 1, uint80(rounds[i]), bytes32(0), bytes32(0), 0, bytes32(0)
            );
            assertEq(
                uint256(guard.sync()),
                uint256(CrossfootGuard.Reason.None),
                string.concat("round ", _u(rounds[i]))
            );
        }
        assertEq(_lastAnswer(guard), 108859885, "round 35 is the reference");

        _post(feed, 36, times[8], answers[8]);
        // The agent's last run saw round 35 on the checked path and attested ALLOW.
        vm.prank(ATTESTER);
        registry.attest(address(feed), 1, 35, bytes32(0), bytes32(0), 24901353, bytes32(0));
        // Without a bound the guard would serve round 36.
        (CrossfootGuard.Reason r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.None), "no bound, recent ALLOW: served");

        // The consumer agent's REVIEW for round 36 arrives (decision 2).
        vm.prank(ATTESTER);
        registry.attest(address(feed), 2, 36, ROUND_36_TX, bytes32(0), 25884405, bytes32(0));
        (r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.AttestationReview), "REVIEW blocks");
        assertEq(
            uint256(guard.sync()), uint256(CrossfootGuard.Reason.AttestationReview), "sync halts"
        );
        assertTrue(_halted(guard), "frozen on the attested path finding");
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector, CrossfootGuard.Reason.Halted, 0, 0
            )
        );
        lender.read();
    }
}
