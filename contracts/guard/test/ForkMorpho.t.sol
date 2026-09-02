// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Fixture} from "./Fixture.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {AggregatorV3Interface} from "../src/interfaces/AggregatorV3Interface.sol";
import {MorphoOracleAdapter} from "../src/adapters/MorphoOracleAdapter.sol";
import {AaveAggregatorAdapter} from "../src/adapters/AaveAggregatorAdapter.sol";
import {Consumer} from "./mocks/Consumer.sol";

interface IMidasFeed is AggregatorV3Interface {
    function maxAnswerDeviation() external view returns (uint256);
}

/// @notice Integration proof on a mainnet fork: the real mRE7 customFeed
/// (config/midas-mainnet.json) wrapped by a guard whose bound follows the feed's own
/// maxAnswerDeviation() at every block (2.0 percent until block 23,520,494, 0.36 percent
/// from it, applied through the guard's timelock), replayed round by round at the pinned
/// blocks of the fixture timeline, with a Morpho Blue oracle adapter and an Aave-style
/// aggregator adapter reading through it. Runs only with CROSSFOOT_FORK_URL set (an
/// archive endpoint) and the `fork` profile:
///   FOUNDRY_PROFILE=fork forge test --match-contract ForkMorpho -vv
contract ForkMorphoTest is Fixture {
    address constant MRE7 = 0x0a2a51f2f206447dE3E3a80FCf92240244722395;
    uint256 constant BOUND_CHANGE_BLOCK = 23520494;
    uint256 constant ROUND_36_BLOCK = 25037959;

    // Rounds 3 to 38 of mRE7.customFeed: blocks from timelines/mre7-customfeed.json of
    // the fixture bundle at 25,884,405. The bound-change block is spliced in order.
    uint256[37] blocks = [
        22297732,
        22525821,
        22740108,
        22933787,
        23181093,
        23426973,
        23434163,
        BOUND_CHANGE_BLOCK,
        23583183,
        23583233,
        23597911,
        23627841,
        23648599,
        23678017,
        23721532,
        23749302,
        23826578,
        23848764,
        23877702,
        23898844,
        23948935,
        24020195,
        24067017,
        24120110,
        24181626,
        24327127,
        24377892,
        24428389,
        24528988,
        24550002,
        24627849,
        24694243,
        24750756,
        24901353,
        25037959,
        25088908,
        25099767
    ];

    IMidasFeed feed = IMidasFeed(MRE7);
    CrossfootGuard guard;
    MorphoOracleAdapter morpho;
    AaveAggregatorAdapter aave;
    Consumer softLender;
    string url;

    event ForkGas(string op, uint256 gas);

    function setUp() public {
        url = vm.envOr("CROSSFOOT_FORK_URL", "");
        if (bytes(url).length == 0) return;
        // Round 2 (block 22,083,676, answer 1e8) is the baseline; the feed's bound there
        // is 2.0 percent.
        vm.createSelectFork(url, 22083676);
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.maxDeviation = uint64(feed.maxAnswerDeviation());
        guard = _deploy(feed, p);
        // mRE7 has 18 token decimals, the feed 8; USDC as the loan token has 6.
        morpho = new MorphoOracleAdapter(guard, 18, 6);
        aave = new AaveAggregatorAdapter(guard);
        softLender = new Consumer(guard);
        vm.prank(address(softLender));
        guard.setConsumerMode(CrossfootGuard.Mode.LastAccepted);
        vm.makePersistent(address(registry));
        vm.makePersistent(address(guard));
        vm.makePersistent(address(morpho));
        vm.makePersistent(address(aave));
        vm.makePersistent(address(softLender));
    }

    function _followFeedBound() internal {
        if (guard.getPendingPolicy().exists) {
            (,, uint64 readyAt,) = _pending();
            if (block.timestamp >= readyAt) {
                vm.prank(OWNER);
                guard.applyPolicy();
            }
        }
        uint256 bound = feed.maxAnswerDeviation();
        if (bound != guard.getPolicy().maxDeviation && !guard.getPendingPolicy().exists) {
            CrossfootGuard.Policy memory p = guard.getPolicy();
            p.maxDeviation = uint64(bound);
            vm.prank(OWNER);
            guard.proposePolicy(p);
        }
    }

    function _pending() internal view returns (uint64, uint64, uint64, bool) {
        CrossfootGuard.PendingPolicy memory pp = guard.getPendingPolicy();
        return (pp.policy.maxDeviation, 0, pp.readyAt, pp.exists);
    }

    function test_fork_mre7_rounds_replay_and_round_36_freezes_the_morpho_price() public {
        vm.skip(bytes(url).length == 0);
        assertEq(
            uint256(guard.getPolicy().maxDeviation), 200_000_000, "2.0 percent at the baseline"
        );
        assertEq(morpho.scaleFactor(), 1e16, "10^(36 + 6 - 18 - 8)");
        assertEq(morpho.price(), uint256(100000000) * 1e16, "baseline price");

        uint256 roundsAccepted;
        for (uint256 i = 0; i < blocks.length; i++) {
            vm.rollFork(blocks[i]);
            _followFeedBound();
            (uint80 roundId, int256 answer,,,) = feed.latestRoundData();
            CrossfootGuard.Reason r = guard.sync();

            if (blocks[i] == BOUND_CHANGE_BLOCK) {
                assertEq(uint256(feed.maxAnswerDeviation()), 36_000_000, "bound changed on chain");
                assertEq(uint256(r), 0, "no new round at the bound-change block");
                continue;
            }
            if (blocks[i] < ROUND_36_BLOCK) {
                assertEq(
                    uint256(r),
                    uint256(CrossfootGuard.Reason.None),
                    string.concat("round ", _u(roundId))
                );
                assertEq(
                    morpho.price(),
                    uint256(answer) * 1e16,
                    "morpho price follows the accepted round"
                );
                assertEq(aave.latestAnswer(), answer, "aave answer follows the accepted round");
                roundsAccepted++;
                if (blocks[i] > BOUND_CHANGE_BLOCK && roundId >= 11) {
                    assertEq(
                        uint256(guard.getPolicy().maxDeviation),
                        36_000_000,
                        "0.36 percent applied after the timelock"
                    );
                }
            } else if (blocks[i] == ROUND_36_BLOCK) {
                assertEq(uint256(roundId), 36, "round 36");
                assertEq(answer, 106438116, "round 36 answer");
                assertEq(uint256(r), uint256(CrossfootGuard.Reason.Deviation), "round 36 rejected");
                (,, CrossfootGuard.Reason haltReason,,) = guard.status();
                assertEq(uint256(haltReason), uint256(CrossfootGuard.Reason.Deviation), "halted");
                vm.expectRevert(
                    abi.encodeWithSelector(
                        CrossfootGuard.GuardRejected.selector, CrossfootGuard.Reason.Halted, 0, 0
                    )
                );
                morpho.price();
                vm.expectRevert(
                    abi.encodeWithSelector(
                        CrossfootGuard.GuardRejected.selector, CrossfootGuard.Reason.Halted, 0, 0
                    )
                );
                aave.latestAnswer();
                // A consumer in last-accepted mode receives round 35 with stale semantics.
                (uint80 rid, int256 served,, uint256 updatedAt, uint80 answeredIn) =
                    softLender.read();
                assertEq(served, 108859885, "round 35 answer");
                assertEq(uint256(rid), 36, "roundId is the source's round 36");
                assertEq(uint256(answeredIn), 35, "answeredInRound 35");
                assertEq(updatedAt, 1776450335, "round 35 time");
            } else {
                assertEq(
                    uint256(r),
                    uint256(CrossfootGuard.Reason.Halted),
                    string.concat("still frozen at round ", _u(roundId))
                );
                vm.expectRevert(
                    abi.encodeWithSelector(
                        CrossfootGuard.GuardRejected.selector, CrossfootGuard.Reason.Halted, 0, 0
                    )
                );
                morpho.price();
            }
        }
        assertEq(roundsAccepted, 33, "rounds 3 to 35 accepted");
        assertEq(_lastAnswer(guard), 108859885, "reference stays at round 35");
    }

    function test_fork_round_36_measured_equals_the_replay_row() public {
        vm.skip(bytes(url).length == 0);
        // Jump straight to round 35 as the reference, then to round 36.
        vm.rollFork(24901353);
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.maxDeviation = uint64(feed.maxAnswerDeviation());
        CrossfootGuard g = _deploy(feed, p);
        MorphoOracleAdapter m = new MorphoOracleAdapter(g, 18, 6);
        vm.makePersistent(address(g));
        vm.makePersistent(address(m));
        assertEq(_lastAnswer(g), 108859885, "round 35 baseline");

        vm.rollFork(ROUND_36_BLOCK);
        (CrossfootGuard.Reason r, uint256 measured, uint256 limit) = _reason(g);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.Deviation), "reason");
        assertEq(measured, 222466613, "deviation_in_force of spec 02 R19");
        assertEq(limit, 36_000_000, "bound_in_force");
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossfootGuard.GuardRejected.selector,
                CrossfootGuard.Reason.Deviation,
                measured,
                limit
            )
        );
        m.price();

        // Gas of the Morpho read through the guard over the live proxy feed, before the
        // rejection: roll back one block so round 35 is current and the read is served.
        vm.rollFork(ROUND_36_BLOCK - 1);
        uint256 before = gasleft();
        uint256 price = m.price();
        uint256 used = before - gasleft();
        assertEq(price, uint256(108859885) * 1e16, "served");
        emit ForkGas("MorphoOracleAdapter.price() cold over the live mRE7 proxy", used);
    }
}
