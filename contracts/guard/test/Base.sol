// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @dev The subset of the Foundry cheatcode interface these tests use. Kept inline so the
/// project has no git submodule; forge-std is not needed for asserts that revert.
interface Vm {
    struct Log {
        bytes32[] topics;
        bytes data;
        address emitter;
    }

    function warp(uint256) external;
    function roll(uint256) external;
    function prank(address) external;
    function startPrank(address) external;
    function stopPrank() external;
    function expectRevert(bytes4) external;
    function expectRevert(bytes calldata) external;
    function recordLogs() external;
    function getRecordedLogs() external returns (Log[] memory);
}

abstract contract Base {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));

    function assertEq(uint256 a, uint256 b, string memory what) internal pure {
        if (a != b) revert(string.concat("assertEq failed: ", what, " ", _u(a), " != ", _u(b)));
    }

    function assertEq(int256 a, int256 b, string memory what) internal pure {
        if (a != b) revert(string.concat("assertEq failed: ", what, " ", _i(a), " != ", _i(b)));
    }

    function assertEq(address a, address b, string memory what) internal pure {
        if (a != b) revert(string.concat("assertEq failed: ", what));
    }

    function assertTrue(bool ok, string memory what) internal pure {
        if (!ok) revert(string.concat("assertTrue failed: ", what));
    }

    function _u(uint256 v) internal pure returns (string memory) {
        if (v == 0) return "0";
        bytes memory buf = new bytes(78);
        uint256 i = 78;
        while (v != 0) {
            buf[--i] = bytes1(uint8(48 + v % 10));
            v /= 10;
        }
        bytes memory out = new bytes(78 - i);
        for (uint256 j = 0; j < out.length; j++) {
            out[j] = buf[i + j];
        }
        return string(out);
    }

    function _i(int256 v) internal pure returns (string memory) {
        return v < 0 ? string.concat("-", _u(uint256(-v))) : _u(uint256(v));
    }
}
