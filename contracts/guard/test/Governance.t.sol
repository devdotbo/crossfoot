// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Fixture} from "./Fixture.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {OwnerPostedFeed} from "./mocks/OwnerPostedFeed.sol";
import {Consumer} from "./mocks/Consumer.sol";

/// @notice Roles, the timelock, consumer modes and the read surface.
contract GovernanceTest is Fixture {
    OwnerPostedFeed feed;
    CrossfootGuard guard;
    Consumer lender;
    uint256 constant T0 = 1_800_000_000;

    function _policy() internal pure returns (CrossfootGuard.Policy memory p) {
        p = _emptyPolicy();
        p.maxDeviation = 10 * ONE_PERCENT;
    }

    function setUp() public {
        feed = new OwnerPostedFeed(8, "X/USD");
        _post(feed, 1, T0, 1e8);
        guard = _deploy(feed, _policy());
        lender = new Consumer(guard);
    }

    function test_guardian_pauses_and_only_the_owner_resumes() public {
        vm.prank(address(0xBAD));
        vm.expectRevert(CrossfootGuard.NotGuardian.selector);
        guard.pause();

        vm.prank(GUARDIAN);
        guard.pause();
        (CrossfootGuard.Reason r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Paused), "paused");
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector, CrossfootGuard.Reason.Paused, 0, 0
            )
        );
        lender.read();
        assertEq(
            uint256(guard.sync()),
            uint256(CrossfootGuard.Reason.Paused),
            "sync is a no-op while paused"
        );

        vm.prank(GUARDIAN);
        vm.expectRevert(CrossfootGuard.NotOwner.selector);
        guard.resume(false);

        vm.prank(OWNER);
        guard.resume(false);
        (r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.None), "resumed");
    }

    function test_policy_changes_wait_for_the_timelock() public {
        CrossfootGuard.Policy memory next = _policy();
        next.maxDeviation = 20 * ONE_PERCENT;

        vm.prank(GUARDIAN);
        vm.expectRevert(CrossfootGuard.NotOwner.selector);
        guard.proposePolicy(next);

        vm.prank(OWNER);
        vm.expectRevert(CrossfootGuard.NothingPending.selector);
        guard.applyPolicy();

        vm.prank(OWNER);
        guard.proposePolicy(next);
        vm.prank(OWNER);
        vm.expectRevert(
            abi.encodeWithSelector(CrossfootGuard.TimelockActive.selector, uint64(T0 + DELAY))
        );
        guard.applyPolicy();

        // A 15 percent post is rejected under the current policy while the change waits.
        _post(feed, 2, T0 + 60, 115_000_000);
        (CrossfootGuard.Reason r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Deviation), "old policy in force");

        vm.warp(T0 + DELAY);
        vm.prank(OWNER);
        guard.applyPolicy();
        assertEq(uint256(guard.getPolicy().maxDeviation), 20 * ONE_PERCENT, "applied");
        assertTrue(!guard.getPendingPolicy().exists, "cleared");
        (r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.None), "served under the new policy");
    }

    function test_role_changes_wait_for_the_timelock_and_cancel_clears_them() public {
        vm.prank(OWNER);
        guard.proposeRoles(address(0x1), address(0x2), address(0x3));
        vm.prank(OWNER);
        vm.expectRevert(
            abi.encodeWithSelector(CrossfootGuard.TimelockActive.selector, uint64(T0 + DELAY))
        );
        guard.applyRoles();

        vm.prank(OWNER);
        guard.cancelProposals();
        vm.warp(T0 + DELAY);
        vm.prank(OWNER);
        vm.expectRevert(CrossfootGuard.NothingPending.selector);
        guard.applyRoles();

        vm.prank(OWNER);
        guard.proposeRoles(address(0x1), address(0x2), address(0x3));
        vm.warp(T0 + 2 * DELAY);
        vm.prank(OWNER);
        guard.applyRoles();
        assertEq(guard.owner(), address(0x1), "owner");
        assertEq(guard.guardian(), address(0x2), "guardian");
        assertEq(guard.attester(), address(0x3), "attester");
        vm.prank(OWNER);
        vm.expectRevert(CrossfootGuard.NotOwner.selector);
        guard.resume(false);
    }

    function test_bad_policies_are_refused() public {
        CrossfootGuard.Policy memory p = _policy();
        p.maxVelocity = 1;
        vm.prank(OWNER);
        vm.expectRevert(CrossfootGuard.BadPolicy.selector);
        guard.proposePolicy(p);

        p = _policy();
        p.attestationMode = 3;
        vm.prank(OWNER);
        vm.expectRevert(CrossfootGuard.BadPolicy.selector);
        guard.proposePolicy(p);

        p = _policy();
        p.minAnswer = 5;
        p.maxAnswer = 4;
        vm.prank(OWNER);
        vm.expectRevert(CrossfootGuard.BadPolicy.selector);
        guard.proposePolicy(p);
    }

    function test_consumer_modes_are_per_caller() public {
        Consumer soft = new Consumer(guard);
        vm.prank(address(soft));
        guard.setConsumerMode(CrossfootGuard.Mode.LastAccepted);
        Consumer other = new Consumer(guard);
        vm.prank(OWNER);
        guard.setConsumerModeFor(address(other), CrossfootGuard.Mode.LastAccepted);
        vm.prank(GUARDIAN);
        vm.expectRevert(CrossfootGuard.NotOwner.selector);
        guard.setConsumerModeFor(address(lender), CrossfootGuard.Mode.LastAccepted);

        _post(feed, 2, T0 + 60, 115_000_000);
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector,
                CrossfootGuard.Reason.Deviation,
                uint256(15 * ONE_PERCENT),
                uint256(10 * ONE_PERCENT)
            )
        );
        lender.read();
        (, int256 a,,,) = soft.read();
        assertEq(a, 1e8, "soft consumer gets the last accepted answer");
        (, int256 b,,,) = other.read();
        assertEq(b, 1e8, "owner-set consumer too");
    }

    function test_default_mode_follows_the_policy_flag() public {
        CrossfootGuard.Policy memory p = _policy();
        p.revertByDefault = false;
        CrossfootGuard g = _deploy(feed, p);
        Consumer c = new Consumer(g);
        _post(feed, 2, T0 + 60, 115_000_000);
        (uint80 roundId, int256 a,,, uint80 answeredInRound) = c.read();
        assertEq(a, 1e8, "last accepted");
        assertEq(uint256(roundId), 2, "source round");
        assertEq(uint256(answeredInRound), 1, "accepted round");
    }

    function test_read_surface_passes_decimals_and_description_through() public view {
        assertEq(uint256(guard.decimals()), 8, "decimals");
        assertEq(
            uint256(keccak256(bytes(guard.description()))),
            uint256(keccak256("CrossfootGuard: X/USD")),
            "description"
        );
        assertEq(guard.version(), 1, "version");
        assertEq(guard.latestAnswer(), 1e8, "latestAnswer");
        assertEq(guard.latestTimestamp(), T0, "latestTimestamp");
        assertEq(guard.latestRound(), 1, "latestRound");
    }

    function test_historical_rounds_are_not_served() public {
        vm.expectRevert(CrossfootGuard.HistoricalRoundsNotGuarded.selector);
        guard.getRoundData(1);
    }

    function test_non_positive_answers_are_refused() public {
        _post(feed, 2, T0 + 60, 0);
        (CrossfootGuard.Reason r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.NonPositive), "zero");
        _post(feed, 3, T0 + 120, -1);
        (r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.NonPositive), "negative");
    }

    function test_baseline_must_be_positive() public {
        OwnerPostedFeed empty = new OwnerPostedFeed(8, "EMPTY");
        vm.expectRevert(CrossfootGuard.BaselineNonPositive.selector);
        _deploy(empty, _policy());
    }

    function test_resume_with_rebase_accepts_the_current_round() public {
        _post(feed, 2, T0 + 60, 150_000_000);
        guard.sync();
        assertTrue(_halted(guard), "halted");
        vm.prank(OWNER);
        guard.resume(true);
        assertEq(_lastAnswer(guard), 150_000_000, "rebased");
        assertEq(uint256(_lastRound(guard)), 2, "round");
        (, int256 a,,,) = lender.read();
        assertEq(a, 150_000_000, "served");
    }

    function test_gas_of_the_guarded_read() public {
        _post(feed, 2, T0 + 60, 101_000_000);
        guard.sync();
        (uint256 used, int256 a) = lender.readGas();
        assertEq(a, 101_000_000, "answer");
        // The exact figure is in the gas snapshot and in docs/specs/10-guard-wrapper.md;
        // this is a wide sanity bound (metering differs between forge releases).
        assertTrue(used < 120_000, string.concat("guarded read gas ", _u(used)));
    }
}

contract SameRoundRewriteTest is Fixture {
    uint256 constant T0 = 1_800_000_000;

    function test_a_rewritten_answer_under_the_same_round_id_is_checked() public {
        OwnerPostedFeed feed = new OwnerPostedFeed(8, "X/USD");
        feed.updatePrice(7, T0, 1e8);
        vm.warp(T0);
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.maxDeviation = 10 * ONE_PERCENT;
        CrossfootGuard g = _deploy(feed, p);
        feed.updatePrice(7, T0, 5e8);
        (CrossfootGuard.Reason r, uint256 measured,) = _reason(g);
        assertEq(
            uint256(r),
            uint256(CrossfootGuard.Reason.Deviation),
            "same round id, new answer, checked"
        );
        assertEq(measured, 400 * ONE_PERCENT, "400 percent");
    }
}
