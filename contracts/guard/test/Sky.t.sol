// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Fixture} from "./Fixture.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {OwnerPostedFeed} from "./mocks/OwnerPostedFeed.sol";

/// @notice The bounded path that passes. The nine SSR changes of the Sky demo window
/// (spec 09 R13; timelines/sky.json of the fixture bundle 23,264,565 to 25,885,408) went
/// through SPBEAM, whose rule at B1 is: 200 to 3000 bps, at most 400 bps per change, at
/// least 57,600 seconds between changes. A guard configured with that same rule, as an
/// absolute delta, min and max and a minimum interval, accepts all nine. A spell-sized
/// jump and a change inside the cooldown are rejected, which is what the setter itself
/// would revert.
contract SkyBoundedPathTest is Fixture {
    OwnerPostedFeed feed;
    CrossfootGuard guard;

    uint256 constant TOC_AT_BASELINE = 1756158203; // SPBEAM toc at B0

    int256[9] bps = [int256(450), 425, 450, 425, 400, 375, 365, 360, 352];
    uint256[9] times = [
        1761582959,
        1762530923,
        1762876091,
        1764689627,
        1765897379,
        1773072635,
        1776865799,
        1779806555,
        1784817803
    ];

    function _policy() internal pure returns (CrossfootGuard.Policy memory p) {
        p = _emptyPolicy();
        p.maxAbsoluteDelta = 400;
        p.minAnswer = 200;
        p.maxAnswer = 3000;
        p.minInterval = 57600;
    }

    function setUp() public {
        feed = new OwnerPostedFeed(0, "SSR bps");
        _post(feed, 1, TOC_AT_BASELINE, 475);
        guard = _deploy(feed, _policy());
    }

    function _replay() internal {
        for (uint256 i = 0; i < 9; i++) {
            _post(feed, i + 2, times[i], bps[i]);
            (CrossfootGuard.Reason r,,) = _reason(guard);
            assertEq(
                uint256(r), uint256(CrossfootGuard.Reason.None), string.concat("change ", _u(i + 1))
            );
            guard.sync();
        }
        assertEq(_lastAnswer(guard), 352, "last SSR");
    }

    function test_nine_spbeam_changes_pass_the_setter_rule() public {
        _replay();
        assertTrue(!_halted(guard), "never halted");
    }

    function test_a_change_over_the_step_is_rejected() public {
        _replay();
        _post(feed, 11, times[8] + 7 days, 800);
        (CrossfootGuard.Reason r, uint256 measured, uint256 limit) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.AbsoluteDelta), "reason");
        assertEq(measured, 448, "448 bps");
        assertEq(limit, 400, "step");
    }

    function test_a_change_inside_the_cooldown_is_rejected() public {
        _replay();
        _post(feed, 11, times[8] + 600, 340);
        (CrossfootGuard.Reason r, uint256 measured, uint256 limit) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Interval), "reason");
        assertEq(measured, 600, "seconds since the previous accepted change");
        assertEq(limit, 57600, "tau");
    }

    function test_a_change_below_the_floor_is_rejected() public {
        _replay();
        _post(feed, 11, times[8] + 7 days, 150);
        (CrossfootGuard.Reason r,,) = _reason(guard);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.OutOfRange), "reason");
    }
}
