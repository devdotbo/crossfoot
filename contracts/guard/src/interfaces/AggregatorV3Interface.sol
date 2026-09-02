// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @notice The Chainlink AggregatorV3 read surface. Every lender adapter named in
/// docs/specs/10-guard-wrapper.md consumes a feed through this interface or through
/// the older AggregatorInterface below.
interface AggregatorV3Interface {
    function decimals() external view returns (uint8);
    function description() external view returns (string memory);
    function version() external view returns (uint256);
    function getRoundData(uint80 roundId)
        external
        view
        returns (uint80, int256, uint256, uint256, uint80);
    function latestRoundData() external view returns (uint80, int256, uint256, uint256, uint80);
}

/// @notice The pre-V3 read surface. Aave v3's AaveOracle reads latestAnswer().
interface AggregatorInterface {
    function latestAnswer() external view returns (int256);
    function latestTimestamp() external view returns (uint256);
    function latestRound() external view returns (uint256);
}

/// @notice The bound getters of a Chainlink OCR aggregator (the contract behind the
/// proxy). A feed whose answer sits on its own floor or ceiling is the Venus 2022 shape.
interface IAggregatorBounds {
    function minAnswer() external view returns (int192);
    function maxAnswer() external view returns (int192);
}
