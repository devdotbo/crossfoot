// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Base} from "./Base.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {CrossfootAttestations} from "../src/CrossfootAttestations.sol";
import {AggregatorV3Interface} from "../src/interfaces/AggregatorV3Interface.sol";
import {OwnerPostedFeed} from "./mocks/OwnerPostedFeed.sol";
import {Consumer} from "./mocks/Consumer.sol";

abstract contract Fixture is Base {
    address internal constant OWNER = address(0xA11CE);
    address internal constant GUARDIAN = address(0x6A2D);
    address internal constant ATTESTER = address(0xA77E5);
    uint64 internal constant DELAY = 2 days;

    uint64 internal constant ONE_PERCENT = 1e8;

    CrossfootAttestations internal registry;

    function _emptyPolicy() internal pure returns (CrossfootGuard.Policy memory p) {
        p.haltOnReject = true;
        p.revertByDefault = true;
    }

    function _deploy(AggregatorV3Interface feed, CrossfootGuard.Policy memory p)
        internal
        returns (CrossfootGuard)
    {
        if (address(registry) == address(0)) registry = new CrossfootAttestations();
        return new CrossfootGuard(feed, registry, p, OWNER, GUARDIAN, ATTESTER, DELAY);
    }

    function _post(OwnerPostedFeed feed, uint256 roundId, uint256 ts, int256 price) internal {
        feed.updatePrice(roundId, ts, price);
        vm.warp(ts);
    }

    function _reason(CrossfootGuard g)
        internal
        view
        returns (CrossfootGuard.Reason r, uint256 measured, uint256 limit)
    {
        CrossfootGuard.Evaluation memory e = g.evaluate();
        return (e.reason, e.measured, e.limit);
    }

    function _stale(CrossfootGuard g) internal view returns (bool) {
        return g.evaluate().stale;
    }

    function _halted(CrossfootGuard g) internal view returns (bool h) {
        (h,,,,) = g.status();
    }

    function _lastAnswer(CrossfootGuard g) internal view returns (int256 a) {
        (a,,,) = g.lastAccepted();
    }

    function _lastRound(CrossfootGuard g) internal view returns (uint80 r) {
        (, r,,) = g.lastAccepted();
    }
}
