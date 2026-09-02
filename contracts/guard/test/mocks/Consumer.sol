// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {
    AggregatorV3Interface,
    AggregatorInterface
} from "../../src/interfaces/AggregatorV3Interface.sol";

/// @dev A lender's price adapter reduced to its read: the guard's mode is keyed by the
/// caller, so the tests read through a contract, as a Morpho oracle, an Aave adapter or a
/// Comet would.
contract Consumer {
    AggregatorV3Interface public immutable feed;

    constructor(AggregatorV3Interface feed_) {
        feed = feed_;
    }

    function read() external view returns (uint80, int256, uint256, uint256, uint80) {
        return feed.latestRoundData();
    }

    function readAnswer() external view returns (int256) {
        return AggregatorInterface(address(feed)).latestAnswer();
    }

    function readGas() external view returns (uint256 used, int256 answer) {
        uint256 before = gasleft();
        (, answer,,,) = feed.latestRoundData();
        used = before - gasleft();
    }
}
