// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Fixture} from "./Fixture.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {BoundedAggregator} from "./mocks/BoundedAggregator.sol";
import {Consumer} from "./mocks/Consumer.sol";

/// @notice The Venus 2022 shape (wiki/cronos-incident-2026.md, prior incidents): the
/// Chainlink LUNA/USD aggregator carried a hardcoded minAnswer of 0.10 USD while the
/// market fell to 0.01; the last number the feed served was on or near its own floor and
/// lenders accepted it. The guard reads the aggregator's own bounds: an answer on the
/// floor is refused as at-bound; an aggregator that stops updating instead is refused as
/// stale. Values are synthetic; only the floor is from the record.
contract VenusFloorTest is Fixture {
    int192 constant FLOOR = 10_000_000; // 0.10 USD at 8 decimals
    int192 constant CEILING = 1_000_000_000_000; // 10,000 USD
    uint256 constant T0 = 1_652_300_000; // 2022-05-11, approximate

    function _policy(address aggregator) internal pure returns (CrossfootGuard.Policy memory p) {
        p = _emptyPolicy();
        p.maxDeviation = 50 * ONE_PERCENT;
        p.maxStaleness = 3600;
        p.boundsSource = aggregator;
    }

    function _fall(BoundedAggregator agg, CrossfootGuard g) internal {
        agg.transmit(20_000_000, T0 + 600);
        vm.warp(T0 + 600);
        assertEq(uint256(g.sync()), uint256(CrossfootGuard.Reason.None), "0.20");
        agg.transmit(12_000_000, T0 + 1200);
        vm.warp(T0 + 1200);
        assertEq(uint256(g.sync()), uint256(CrossfootGuard.Reason.None), "0.12");
        agg.transmit(10_700_000, T0 + 1800);
        vm.warp(T0 + 1800);
        assertEq(uint256(g.sync()), uint256(CrossfootGuard.Reason.None), "0.107, above the floor");
    }

    function test_a_clamped_answer_on_the_floor_is_refused_as_at_bound() public {
        BoundedAggregator agg =
            new BoundedAggregator(FLOOR, CEILING, BoundedAggregator.Behaviour.CLAMP);
        agg.transmit(30_000_000, T0);
        vm.warp(T0);
        CrossfootGuard g = _deploy(agg, _policy(address(agg)));
        Consumer lender = new Consumer(g);
        _fall(agg, g);

        agg.transmit(5_000_000, T0 + 2400); // the market is at 0.05; the aggregator stores 0.10
        vm.warp(T0 + 2400);
        assertEq(agg.answer(), int256(FLOOR), "stored on the floor");
        (CrossfootGuard.Reason r, uint256 measured, uint256 limit) = _reason(g);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.AtSourceBound), "at bound");
        assertEq(measured, uint256(int256(FLOOR)), "answer");
        assertEq(limit, uint256(int256(FLOOR)), "floor");
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector,
                CrossfootGuard.Reason.AtSourceBound,
                measured,
                limit
            )
        );
        lender.read();
        assertEq(uint256(g.sync()), uint256(CrossfootGuard.Reason.AtSourceBound), "halts");
    }

    function test_an_aggregator_that_stops_below_its_floor_is_refused_as_stale() public {
        BoundedAggregator agg =
            new BoundedAggregator(FLOOR, CEILING, BoundedAggregator.Behaviour.REJECT);
        agg.transmit(30_000_000, T0);
        vm.warp(T0);
        CrossfootGuard g = _deploy(agg, _policy(address(agg)));
        Consumer lender = new Consumer(g);
        _fall(agg, g);

        vm.expectRevert("median is out of min-max range");
        agg.transmit(5_000_000, T0 + 2400);
        // The feed keeps serving 0.107 with an ageing timestamp; after maxStaleness the
        // guard refuses it while the source still answers.
        vm.warp(T0 + 1800 + 3601);
        assertTrue(_stale(g), "stale");
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardStale.selector, uint80(4), uint256(T0 + 1800), uint256(3600)
            )
        );
        lender.read();
    }
}
