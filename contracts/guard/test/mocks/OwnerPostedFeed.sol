// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {AggregatorV3Interface} from "../../src/interfaces/AggregatorV3Interface.sol";

/// @dev The Tectonic shape (raw/cronos-tonic-oracle-chain-reads-2026-09-01.md): an
/// AggregatorV3-shaped feed with one owner and exactly one setter,
/// updatePrice(uint256 roundId, uint256 timestamp, int256 price), no bound getter, no
/// minimum interval. The Midas customFeed is the same shape with a round counter; the
/// tests post its rounds through the same setter and note the path in the test.
contract OwnerPostedFeed is AggregatorV3Interface {
    uint8 private immutable _decimals;
    string private _description;
    address public owner;

    uint80 public roundId;
    int256 public answer;
    uint256 public updatedAt;

    event PriceUpdated(uint256 indexed roundId, uint256 indexed timestamp, int256 indexed price);

    constructor(uint8 decimals_, string memory description_) {
        _decimals = decimals_;
        _description = description_;
        owner = msg.sender;
    }

    function updatePrice(uint256 roundId_, uint256 timestamp_, int256 price) external {
        require(msg.sender == owner, "not owner");
        roundId = uint80(roundId_);
        answer = price;
        updatedAt = timestamp_;
        emit PriceUpdated(roundId_, timestamp_, price);
    }

    function decimals() external view returns (uint8) {
        return _decimals;
    }

    function description() external view returns (string memory) {
        return _description;
    }

    function version() external pure returns (uint256) {
        return 0;
    }

    function getRoundData(uint80) external view returns (uint80, int256, uint256, uint256, uint80) {
        return (roundId, answer, updatedAt, updatedAt, roundId);
    }

    function latestRoundData() external view returns (uint80, int256, uint256, uint256, uint80) {
        return (roundId, answer, updatedAt, updatedAt, roundId);
    }
}
