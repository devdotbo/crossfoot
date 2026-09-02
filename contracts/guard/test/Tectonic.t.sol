// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Fixture} from "./Fixture.sol";
import {Vm} from "./Base.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {OwnerPostedFeed} from "./mocks/OwnerPostedFeed.sol";
import {Consumer} from "./mocks/Consumer.sol";

/// @notice Replays the TONIC/USD post series of 2026-08-30 through a guard with a policy a
/// TONIC lender could have held. Values and times: the five canonical-chain posts from
/// raw/cronos-tonic-oracle-chain-reads-2026-09-01.md (own eth_getLogs) and the three
/// attack-time posts from MASTR's archive reconstruction as relayed in
/// wiki/cronos-incident-2026.md (the blocks were discarded by the rollback; the values
/// are unverified against the chain). 12 decimals, as the feed.
///
/// The policy is deliberately wide because the legitimate series is wide: the five posts
/// before the attack moved -14.2, -8.0, -7.2 and +2.3 percent, 26.7 percent from top to
/// bottom in 25 minutes. A bound that rejects those is not a policy TONIC's own lender
/// could have run. 25 percent per post and 50 percent per hour accept every one of them
/// and reject the first attack post by a factor of twenty.
contract TectonicTest is Fixture {
    uint256 constant T_11_49_42 = 1788090582;
    uint256 constant T_12_07_31 = 1788091651;
    uint256 constant T_12_09_37 = 1788091777;
    uint256 constant T_12_14_37 = 1788092077;
    uint256 constant T_12_19_37 = 1788092377;
    uint256 constant T_12_39_10 = 1788093550; // first attack-time post, 6.46x
    uint256 constant T_12_44_08 = 1788093848; // 13.4x
    uint256 constant T_12_49_13 = 1788094153; // 2.26x
    uint256 constant T_12_49_39 = 1788094179; // the drain transaction
    uint256 constant T_RESTART_POST = 1788187906; // 2026-08-31 14:51:46, first post after the restart

    OwnerPostedFeed feed;
    CrossfootGuard guard;
    Consumer lender;

    function _rid(uint256 ts) internal pure returns (uint256) {
        // The feed uses a millisecond-scale timestamp as its round id.
        return ts * 1_000_000;
    }

    function _policy() internal pure returns (CrossfootGuard.Policy memory p) {
        p = _emptyPolicy();
        p.maxDeviation = 25 * ONE_PERCENT;
        p.maxVelocity = 50 * ONE_PERCENT;
        p.velocityWindow = 3600;
        p.maxStaleness = 3600;
    }

    function setUp() public {
        feed = new OwnerPostedFeed(12, "TONIC/USD ORACLE");
        _post(feed, _rid(T_11_49_42), T_11_49_42, 14163);
        guard = _deploy(feed, _policy());
        lender = new Consumer(guard);
    }

    function _postAndExpectAccepted(uint256 ts, int256 price) internal {
        _post(feed, _rid(ts), ts, price);
        (CrossfootGuard.Reason r, uint256 measured,) = _reason(guard);
        assertEq(
            uint256(r), uint256(CrossfootGuard.Reason.None), string.concat("reason at ", _u(ts))
        );
        assertEq(measured, 0, "measured");
        assertEq(uint256(guard.sync()), uint256(CrossfootGuard.Reason.None), "sync");
        (, int256 served,,,) = lender.read();
        assertEq(served, price, "served value after accept");
    }

    function _replayPreAttack() internal {
        _postAndExpectAccepted(T_12_07_31, 12154);
        _postAndExpectAccepted(T_12_09_37, 11187);
        _postAndExpectAccepted(T_12_14_37, 10383);
        _postAndExpectAccepted(T_12_19_37, 10622);
    }

    function test_the_five_ordinary_posts_pass_the_policy() public {
        _replayPreAttack();
        assertEq(_lastAnswer(guard), 10622, "reference is the 12:19:37 value");
        // The window anchor is the 11:49:42 post; the series moved 25.0 percent against
        // it at 12:19:37, within the 50 percent velocity limit.
        (int256 anchorAnswer, uint64 openedAt) = guard.anchor();
        assertEq(anchorAnswer, 14163, "anchor answer");
        assertEq(uint256(openedAt), T_11_49_42, "anchor opened");
    }

    function test_the_first_attack_post_is_rejected_on_deviation_and_halts() public {
        _replayPreAttack();
        _post(feed, _rid(T_12_39_10), T_12_39_10, 68593);

        // Read path before anyone records the rejection: the value is refused already.
        (CrossfootGuard.Reason r, uint256 measured, uint256 limit) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Deviation), "reason");
        assertEq(measured, 54576350969, "545.76 percent at the 1e8 percent scale");
        assertEq(measured, uint256(57971) * 1e10 / 10622, "formula");
        assertEq(limit, 25 * ONE_PERCENT, "limit");
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector,
                CrossfootGuard.Reason.Deviation,
                measured,
                limit
            )
        );
        lender.read();

        // Recording the rejection freezes the guard.
        vm.recordLogs();
        assertEq(
            uint256(guard.sync()),
            uint256(CrossfootGuard.Reason.Deviation),
            "sync returns the reason"
        );
        assertTrue(_halted(guard), "halted");
        (,, CrossfootGuard.Reason haltReason, uint80 haltRound,) = guard.status();
        assertEq(uint256(haltReason), uint256(CrossfootGuard.Reason.Deviation), "halt reason");
        assertEq(uint256(haltRound), _rid(T_12_39_10), "halt round");
        Vm.Log[] memory logs = vm.getRecordedLogs();
        assertEq(logs.length, 2, "RoundRejected and Halted");
        assertEq(
            uint256(logs[0].topics[0]),
            uint256(keccak256("RoundRejected(uint80,int256,uint8,uint256,uint256)")),
            "RoundRejected"
        );
        assertEq(uint256(logs[1].topics[0]), uint256(keccak256("Halted(uint8,uint80)")), "Halted");

        // Frozen: the default consumer reverts on every read from here on.
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector, CrossfootGuard.Reason.Halted, 0, 0
            )
        );
        lender.read();
        assertEq(_lastAnswer(guard), 10622, "reference unchanged");
    }

    function test_the_second_and_third_posts_stay_rejected_while_frozen() public {
        _replayPreAttack();
        _post(feed, _rid(T_12_39_10), T_12_39_10, 68593);
        guard.sync();
        _post(feed, _rid(T_12_44_08), T_12_44_08, 918893);
        assertEq(uint256(guard.sync()), uint256(CrossfootGuard.Reason.Halted), "12:44:08 halted");
        _post(feed, _rid(T_12_49_13), T_12_49_13, 2076321);
        assertEq(uint256(guard.sync()), uint256(CrossfootGuard.Reason.Halted), "12:49:13 halted");
        vm.warp(T_12_49_39);
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector, CrossfootGuard.Reason.Halted, 0, 0
            )
        );
        lender.read();
        assertEq(_lastAnswer(guard), 10622, "the reference never moved");
    }

    function test_without_halting_every_attack_post_is_still_rejected_against_the_reference()
        public
    {
        CrossfootGuard.Policy memory p = _policy();
        p.haltOnReject = false;
        guard = _deploy(feed, p);
        lender = new Consumer(guard);
        _replayPreAttack();

        _post(feed, _rid(T_12_39_10), T_12_39_10, 68593);
        assertEq(uint256(guard.sync()), uint256(CrossfootGuard.Reason.Deviation), "post 1");
        _post(feed, _rid(T_12_44_08), T_12_44_08, 918893);
        (CrossfootGuard.Reason r, uint256 measured,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Deviation), "post 2");
        assertEq(
            measured,
            uint256(908271) * 1e10 / 10622,
            "post 2 measured against 10622, not against post 1"
        );
        assertEq(uint256(guard.sync()), uint256(CrossfootGuard.Reason.Deviation), "post 2 sync");
        _post(feed, _rid(T_12_49_13), T_12_49_13, 2076321);
        assertEq(uint256(guard.sync()), uint256(CrossfootGuard.Reason.Deviation), "post 3");
        assertTrue(!_halted(guard), "not halted by policy");
        assertEq(_lastAnswer(guard), 10622, "reference");
    }

    function test_a_stale_mode_consumer_receives_the_last_accepted_answer_with_stale_semantics()
        public
    {
        _replayPreAttack();
        Consumer soft = new Consumer(guard);
        vm.prank(address(soft));
        guard.setConsumerMode(CrossfootGuard.Mode.LastAccepted);

        _post(feed, _rid(T_12_39_10), T_12_39_10, 68593);
        guard.sync();
        (
            uint80 roundId,
            int256 answer,
            uint256 startedAt,
            uint256 updatedAt,
            uint80 answeredInRound
        ) = soft.read();
        assertEq(answer, 10622, "last accepted answer");
        assertEq(uint256(roundId), _rid(T_12_39_10), "roundId is the source's latest round");
        assertEq(
            uint256(answeredInRound), _rid(T_12_19_37), "answeredInRound is the accepted round"
        );
        assertTrue(answeredInRound < roundId, "Chainlink stale convention");
        assertEq(updatedAt, T_12_19_37, "updatedAt is the accepted round's time");
        assertEq(startedAt, T_12_19_37, "startedAt");
        assertEq(soft.readAnswer(), 10622, "latestAnswer for the Aave read");
    }

    function test_the_first_post_after_the_restart_needs_the_owner_to_rebase() public {
        _replayPreAttack();
        _post(feed, _rid(T_12_39_10), T_12_39_10, 68593);
        guard.sync();

        // 26.5 hours later the canonical chain carries 2.8388e-8, 2.67 times the reference.
        _post(feed, _rid(T_RESTART_POST), T_RESTART_POST, 28388);
        vm.prank(OWNER);
        guard.resume(false);
        (CrossfootGuard.Reason r, uint256 measured,) = _reason(guard);
        assertEq(
            uint256(r),
            uint256(CrossfootGuard.Reason.Deviation),
            "still over the bound against 10622"
        );
        assertEq(measured, uint256(17766) * 1e10 / 10622, "167 percent");

        // After review the owner accepts the current round as the new reference.
        vm.prank(OWNER);
        guard.resume(true);
        assertEq(_lastAnswer(guard), 28388, "rebased");
        (r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.None), "served again");
        (, int256 served,,,) = lender.read();
        assertEq(served, 28388, "served");
    }

    function test_silence_after_the_last_accepted_post_reads_as_stale() public {
        _replayPreAttack();
        vm.warp(T_12_19_37 + 3601);
        assertTrue(_stale(guard), "stale after maxStaleness");
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardStale.selector,
                uint80(_rid(T_12_19_37)),
                T_12_19_37,
                uint256(3600)
            )
        );
        lender.read();
        assertTrue(!_halted(guard), "staleness never halts");
    }

    /// @notice The velocity limit is what catches a series of steps that each pass the
    /// per-post bound. Synthetic: +20 percent every five minutes under a 25 percent bound
    /// and a 50 percent per hour limit; the third step is 72.8 percent from the anchor.
    function test_velocity_rejects_a_ramp_of_in_bound_steps() public {
        OwnerPostedFeed ramp = new OwnerPostedFeed(8, "RAMP");
        uint256 t0 = 1_800_000_000;
        _post(ramp, 1, t0, 10000);
        CrossfootGuard g = _deploy(ramp, _policy());
        _post(ramp, 2, t0 + 300, 12000);
        assertEq(uint256(g.sync()), uint256(CrossfootGuard.Reason.None), "step 1");
        _post(ramp, 3, t0 + 600, 14400);
        assertEq(uint256(g.sync()), uint256(CrossfootGuard.Reason.None), "step 2");
        _post(ramp, 4, t0 + 900, 17280);
        (CrossfootGuard.Reason r, uint256 measured, uint256 limit) = _reason(g);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Velocity), "step 3 rejected on velocity");
        assertEq(measured, uint256(7280) * 1e10 / 10000, "72.8 percent from the anchor");
        assertEq(limit, 50 * ONE_PERCENT, "limit");
        assertEq(uint256(g.sync()), uint256(CrossfootGuard.Reason.Velocity), "halts");
        assertTrue(_halted(g), "halted");
    }
}
