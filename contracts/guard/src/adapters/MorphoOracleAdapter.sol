// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {CrossfootGuard} from "../CrossfootGuard.sol";

/// @notice Morpho Blue's oracle interface: the price of one unit of collateral token in
/// loan token units, scaled by 1e36, adjusted for both tokens' decimals.
interface IOracle {
    function price() external view returns (uint256);
}

/// @title MorphoOracleAdapter
/// @notice The Morpho Blue read over a CrossfootGuard, reduced to one feed: the shape of
/// MorphoChainlinkOracleV2 with baseFeed1 = guard and no other feeds or vaults. Morpho's
/// data-feed library checks only that the answer is non-negative and reads no timestamp,
/// so the guard should be in Revert mode for a Morpho consumer (spec 10, integration
/// notes): a refused round reverts price(), which makes borrow, withdrawCollateral and
/// liquidate revert; supply, supplyCollateral and repay never read the oracle.
contract MorphoOracleAdapter is IOracle {
    CrossfootGuard public immutable guard;
    /// @dev 10 ** (36 + loanTokenDecimals - collateralTokenDecimals - feedDecimals), as
    /// MorphoChainlinkOracleV2 computes SCALE_FACTOR for one base feed.
    uint256 public immutable scaleFactor;

    error NonPositiveAnswer();
    error ScaleUnderflow();

    constructor(CrossfootGuard guard_, uint8 collateralTokenDecimals, uint8 loanTokenDecimals) {
        guard = guard_;
        uint8 feedDecimals = guard_.decimals();
        int256 exponent = int256(36) + int256(uint256(loanTokenDecimals))
            - int256(uint256(collateralTokenDecimals)) - int256(uint256(feedDecimals));
        if (exponent < 0) revert ScaleUnderflow();
        scaleFactor = 10 ** uint256(exponent);
    }

    function price() external view override returns (uint256) {
        (, int256 answer,,,) = guard.latestRoundData();
        if (answer <= 0) revert NonPositiveAnswer();
        return uint256(answer) * scaleFactor;
    }
}
