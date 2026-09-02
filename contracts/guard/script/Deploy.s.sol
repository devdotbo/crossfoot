// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Vm} from "../test/Base.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {CrossfootAttestations} from "../src/CrossfootAttestations.sol";
import {AggregatorV3Interface} from "../src/interfaces/AggregatorV3Interface.sol";
import {OwnerPostedFeed} from "../test/mocks/OwnerPostedFeed.sol";

/// @notice Testnet deployment (docs/specs/10-guard-wrapper.md, appendix). Deploys the
/// attestation registry and one guard over either a live feed (`FEED` set) or a fresh
/// owner-posted mock seeded with `MOCK_ANSWER`. Everything is read from the environment;
/// nothing is hard coded. Roles default to the broadcaster.
///
///   FEED=<address or unset> OWNER=... GUARDIAN=... ATTESTER=... \
///   forge script script/Deploy.s.sol --rpc-url $RPC --private-key $PK --broadcast
contract Deploy {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));
    bool public IS_SCRIPT = true;

    event Deployed(address registry, address feed, address guard, bool mockFeed);

    function run()
        external
        returns (CrossfootAttestations registry, AggregatorV3Interface feed, CrossfootGuard guard)
    {
        address deployer = vm.envAddress("DEPLOYER");
        address owner = vm.envOr("OWNER", deployer);
        address guardian = vm.envOr("GUARDIAN", deployer);
        address attester = vm.envOr("ATTESTER", deployer);
        address feedAddress = vm.envOr("FEED", address(0));
        uint64 delay = uint64(vm.envOr("TIMELOCK_DELAY", uint256(1 hours)));

        CrossfootGuard.Policy memory p;
        p.maxDeviation = uint64(vm.envOr("MAX_DEVIATION", uint256(200_000_000))); // 2.0 percent
        p.maxVelocity = uint64(vm.envOr("MAX_VELOCITY", uint256(0)));
        p.velocityWindow = uint32(vm.envOr("VELOCITY_WINDOW", uint256(0)));
        p.maxStaleness = uint32(vm.envOr("MAX_STALENESS", uint256(0)));
        p.minInterval = uint32(vm.envOr("MIN_INTERVAL", uint256(0)));
        p.haltOnReject = vm.envOr("HALT_ON_REJECT", true);
        p.revertByDefault = vm.envOr("REVERT_BY_DEFAULT", true);

        vm.startBroadcast();
        registry = new CrossfootAttestations();
        bool mockFeed = feedAddress == address(0);
        if (mockFeed) {
            OwnerPostedFeed mock = new OwnerPostedFeed(
                uint8(vm.envOr("MOCK_DECIMALS", uint256(8))), "CrossfootGuard mock feed"
            );
            mock.updatePrice(
                1, block.timestamp, int256(vm.envOr("MOCK_ANSWER", uint256(100_000_000)))
            );
            feed = mock;
        } else {
            feed = AggregatorV3Interface(feedAddress);
        }
        guard = new CrossfootGuard(feed, registry, p, owner, guardian, attester, delay);
        vm.stopBroadcast();
        emit Deployed(address(registry), address(feed), address(guard), mockFeed);
    }
}
