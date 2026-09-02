// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Fixture} from "./Fixture.sol";
import {CrossfootGuard} from "../src/CrossfootGuard.sol";
import {CrossfootAttestations} from "../src/CrossfootAttestations.sol";
import {OwnerPostedFeed} from "./mocks/OwnerPostedFeed.sol";

contract AttestationsTest is Fixture {
    OwnerPostedFeed feed;
    uint256 constant T0 = 1_800_000_000;

    function setUp() public {
        registry = new CrossfootAttestations();
        feed = new OwnerPostedFeed(8, "X/USD");
        _post(feed, 1, T0, 1e8);
    }

    function test_attest_stores_per_attester_and_feed() public {
        vm.prank(address(0x1));
        registry.attest(
            address(feed),
            1,
            1,
            bytes32(uint256(0xAA)),
            bytes32(uint256(0xBB)),
            100,
            bytes32(uint256(0xCC))
        );
        vm.prank(address(0x2));
        registry.attest(address(feed), 2, 1, bytes32(uint256(0xDD)), bytes32(0), 101, bytes32(0));
        CrossfootAttestations.Record memory a = registry.latest(address(0x1), address(feed));
        CrossfootAttestations.Record memory b = registry.latest(address(0x2), address(feed));
        assertEq(uint256(a.decision), 1, "a decision");
        assertEq(uint256(a.recordHash), 0xAA, "a record hash");
        assertEq(uint256(a.deploymentDigest), 0xBB, "a digest");
        assertEq(uint256(a.sourceBlock), 100, "a block");
        assertEq(uint256(a.bundleRoot), 0xCC, "a bundle");
        assertEq(uint256(a.attestedAt), T0, "a time");
        assertEq(uint256(b.decision), 2, "b decision");
        assertEq(uint256(b.recordHash), 0xDD, "b record hash");
        (uint8 d, uint80 covered, uint64 at) = registry.decisionFor(address(0x2), address(feed));
        assertEq(uint256(d), 2, "decisionFor");
        assertEq(uint256(covered), 1, "covered");
        assertEq(uint256(at), T0, "at");
    }

    function test_bad_decisions_revert() public {
        vm.expectRevert(CrossfootAttestations.BadDecision.selector);
        registry.attest(address(feed), 0, 1, bytes32(0), bytes32(0), 0, bytes32(0));
        vm.expectRevert(CrossfootAttestations.BadDecision.selector);
        registry.attest(address(feed), 3, 1, bytes32(0), bytes32(0), 0, bytes32(0));
    }

    function test_per_round_mode_needs_an_allow_covering_the_round() public {
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.attestationMode = 2;
        p.maxAttestationAge = 1 days;
        CrossfootGuard g = _deploy(feed, p);

        _post(feed, 2, T0 + 60, 101_000_000);
        (CrossfootGuard.Reason r,,) = _reason(g);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.AttestationMissing), "no record");

        vm.prank(ATTESTER);
        registry.attest(address(feed), 1, 1, bytes32(0), bytes32(0), 1, bytes32(0));
        (r,,) = _reason(g);
        assertEq(
            uint256(r),
            uint256(CrossfootGuard.Reason.AttestationMissing),
            "record covers round 1 only"
        );

        vm.prank(ATTESTER);
        registry.attest(address(feed), 1, 2, bytes32(0), bytes32(0), 2, bytes32(0));
        (r,,) = _reason(g);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.None), "covered");
        assertEq(uint256(g.sync()), uint256(CrossfootGuard.Reason.None), "accepted");

        // An accepted round keeps serving when the attester goes quiet.
        vm.warp(T0 + 60 + 2 days);
        (r,,) = _reason(g);
        assertEq(
            uint256(r), uint256(CrossfootGuard.Reason.None), "accepted round unaffected by age"
        );
        // A new round does not.
        _post(feed, 3, T0 + 60 + 2 days, 101_500_000);
        (r,,) = _reason(g);
        assertEq(
            uint256(r),
            uint256(CrossfootGuard.Reason.AttestationStale),
            "stale record for a new round"
        );
    }

    function test_review_blocks_mode_does_not_need_per_round_coverage() public {
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.attestationMode = 1;
        p.maxAttestationAge = 1 days;
        CrossfootGuard g = _deploy(feed, p);
        vm.prank(ATTESTER);
        registry.attest(address(feed), 1, 1, bytes32(0), bytes32(0), 1, bytes32(0));
        _post(feed, 2, T0 + 60, 101_000_000);
        (CrossfootGuard.Reason r,,) = _reason(g);
        assertEq(uint256(r), uint256(CrossfootGuard.Reason.None), "recent ALLOW is enough");
    }

    function test_another_attesters_review_is_ignored() public {
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.attestationMode = 1;
        p.maxAttestationAge = 1 days;
        CrossfootGuard g = _deploy(feed, p);
        vm.prank(ATTESTER);
        registry.attest(address(feed), 1, 1, bytes32(0), bytes32(0), 1, bytes32(0));
        vm.prank(address(0xBAD));
        registry.attest(address(feed), 2, 1, bytes32(0), bytes32(0), 1, bytes32(0));
        (CrossfootGuard.Reason r,,) = _reason(g);
        assertEq(
            uint256(r), uint256(CrossfootGuard.Reason.None), "only the configured attester counts"
        );
    }

    function test_attestation_mode_needs_a_registry() public {
        CrossfootGuard.Policy memory p = _emptyPolicy();
        p.attestationMode = 1;
        vm.expectRevert(CrossfootGuard.BadPolicy.selector);
        new CrossfootGuard(
            feed, CrossfootAttestations(address(0)), p, OWNER, GUARDIAN, ATTESTER, DELAY
        );
    }
}
