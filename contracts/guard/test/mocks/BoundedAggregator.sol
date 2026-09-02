// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {
    AggregatorV3Interface,
    IAggregatorBounds
} from "../../src/interfaces/AggregatorV3Interface.sol";

/// @dev A Chainlink-style OCR aggregator with a hardcoded minAnswer and maxAnswer, the
/// Venus 2022 shape. Two behaviours are modelled because accounts of the LUNA incident
/// describe both: `REJECT` refuses a transmission outside the range so the feed stops
/// updating (OCR2Aggregator's "median is out of min-max range"), `CLAMP` stores the floor
/// or ceiling. Under either the guard refuses: stale under REJECT, at-bound under CLAMP.
contract BoundedAggregator is AggregatorV3Interface, IAggregatorBounds {
    enum Behaviour {
        REJECT,
        CLAMP
    }

    int192 private immutable _min;
    int192 private immutable _max;
    Behaviour public behaviour;

    uint80 public roundId;
    int256 public answer;
    uint256 public updatedAt;

    constructor(int192 min_, int192 max_, Behaviour behaviour_) {
        _min = min_;
        _max = max_;
        behaviour = behaviour_;
    }

    function transmit(int256 median, uint256 timestamp) external {
        if (median < _min || median > _max) {
            if (behaviour == Behaviour.REJECT) revert("median is out of min-max range");
            median = median < _min ? int256(_min) : int256(_max);
        }
        roundId += 1;
        answer = median;
        updatedAt = timestamp;
    }

    function minAnswer() external view returns (int192) {
        return _min;
    }

    function maxAnswer() external view returns (int192) {
        return _max;
    }

    function decimals() external pure returns (uint8) {
        return 8;
    }

    function description() external pure returns (string memory) {
        return "LUNA / USD";
    }

    function version() external pure returns (uint256) {
        return 4;
    }

    function getRoundData(uint80) external view returns (uint80, int256, uint256, uint256, uint80) {
        return (roundId, answer, updatedAt, updatedAt, roundId);
    }

    function latestRoundData() external view returns (uint80, int256, uint256, uint256, uint80) {
        return (roundId, answer, updatedAt, updatedAt, roundId);
    }
}
