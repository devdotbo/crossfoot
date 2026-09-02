// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {CrossfootGuard} from "../CrossfootGuard.sol";

/// @notice The read AaveOracle makes on an asset source: latestAnswer() in the base
/// currency's decimals (8 for USD). AaveOracle falls back to its fallback oracle only on
/// a non-positive answer; a revert propagates, which is the intended behaviour under a
/// guard in Revert mode.
interface IChainlinkAggregator {
    function latestAnswer() external view returns (int256);
    function latestTimestamp() external view returns (uint256);
    function decimals() external view returns (uint8);
}

/// @title AaveAggregatorAdapter
/// @notice Rescales a CrossfootGuard's answer to Aave's 8 base-currency decimals. A
/// CAPO-style adapter is the same shape with one more rule; this one only rescales.
contract AaveAggregatorAdapter is IChainlinkAggregator {
    CrossfootGuard public immutable guard;
    uint8 public constant BASE_DECIMALS = 8;
    uint8 public immutable feedDecimals;

    constructor(CrossfootGuard guard_) {
        guard = guard_;
        feedDecimals = guard_.decimals();
    }

    function latestAnswer() external view override returns (int256) {
        (, int256 answer,,,) = guard.latestRoundData();
        if (feedDecimals == BASE_DECIMALS) return answer;
        if (feedDecimals > BASE_DECIMALS) {
            return answer / int256(10 ** (feedDecimals - BASE_DECIMALS));
        }
        return answer * int256(10 ** (BASE_DECIMALS - feedDecimals));
    }

    function latestTimestamp() external view override returns (uint256) {
        (,,, uint256 updatedAt,) = guard.latestRoundData();
        return updatedAt;
    }

    function decimals() external pure override returns (uint8) {
        return BASE_DECIMALS;
    }
}
