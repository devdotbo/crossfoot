// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Fixture} from "./Fixture.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {OwnerPostedFeed} from "./mocks/OwnerPostedFeed.sol";
import {Consumer} from "./mocks/Consumer.sol";

/// @notice Cold-path gas of the three operations a lender pays for. Each test performs
/// exactly one measured call after setUp, so storage and the source are cold. The figures
/// are emitted as events (read them with `forge test --match-contract Gas -vvv`) and
/// bounded by asserts so a regression fails the suite; the reference numbers are recorded
/// in docs/specs/10-guard-wrapper.md.
contract GasTest is Fixture {
    event GasMeasured(string op, uint256 gas);

    OwnerPostedFeed feed;
    CrossfootGuard guard;
    CrossfootGuard attested;
    Consumer lender;
    Consumer attestedLender;
    uint256 constant T0 = 1_800_000_000;

    function setUp() public {
        feed = new OwnerPostedFeed(8, "X/USD");
        feed.updatePrice(1, T0, 1e8);
        vm.warp(T0);
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.maxDeviation = 10 * ONE_PERCENT;
        p.maxVelocity = 30 * ONE_PERCENT;
        p.velocityWindow = 3600;
        p.maxStaleness = 3600;
        guard = _deploy(feed, p);
        lender = new Consumer(guard);

        p.attestationMode = 1;
        p.maxAttestationAge = 1 days;
        attested = _deploy(feed, p);
        attestedLender = new Consumer(attested);
        vm.prank(ATTESTER);
        registry.attest(address(feed), 1, 1, bytes32(0), bytes32(0), 1, bytes32(0));

        // A new in-bound round is waiting for every test.
        feed.updatePrice(2, T0 + 60, 101_000_000);
        vm.warp(T0 + 60);
    }

    function test_gas_cold_guarded_read() public {
        (uint256 used, int256 a) = lender.readGas();
        assertEq(a, 101_000_000, "answer");
        emit GasMeasured("latestRoundData cold, new round, deviation and velocity", used);
        assertTrue(used < 45_000, _u(used));
    }

    function test_gas_cold_guarded_read_with_attestation() public {
        (uint256 used, int256 a) = attestedLender.readGas();
        assertEq(a, 101_000_000, "answer");
        emit GasMeasured("latestRoundData cold, attestation mode 1", used);
        assertTrue(used < 55_000, _u(used));
    }

    function test_gas_cold_sync_accept() public {
        uint256 before = gasleft();
        CrossfootGuard.Reason r = guard.sync();
        uint256 used = before - gasleft();
        assertEq(uint256(r), 0, "accepted");
        emit GasMeasured("sync cold, accept", used);
        assertTrue(used < 60_000, _u(used));
    }

    function test_gas_cold_sync_reject_and_halt() public {
        feed.updatePrice(3, T0 + 120, 200_000_000);
        vm.warp(T0 + 120);
        uint256 before = gasleft();
        CrossfootGuard.Reason r = guard.sync();
        uint256 used = before - gasleft();
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Deviation), "rejected");
        emit GasMeasured("sync cold, reject and halt", used);
        assertTrue(used < 80_000, _u(used));
    }

    function test_gas_cold_read_while_halted_last_accepted_mode() public {
        feed.updatePrice(3, T0 + 120, 200_000_000);
        vm.warp(T0 + 120);
        guard.sync();
        Consumer soft = new Consumer(guard);
        vm.prank(address(soft));
        guard.setConsumerMode(CrossfootGuard.Mode.LastAccepted);
        (uint256 used, int256 a) = soft.readGas();
        assertEq(a, 1e8, "last accepted");
        emit GasMeasured("latestRoundData while halted, last accepted mode (warm)", used);
    }
}
