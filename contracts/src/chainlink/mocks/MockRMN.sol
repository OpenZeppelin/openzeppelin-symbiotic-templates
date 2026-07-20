// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

import {IRMN} from "@chainlink/contracts-ccip/contracts/interfaces/IRMN.sol";

/// @title MockRMN
/// @notice Configurable RMN curse state for verifier tests and local deployments.
contract MockRMN is IRMN {
    mapping(bytes16 subject => bool cursed) private s_cursed;
    mapping(bytes16 subject => bool tracked) private s_tracked;
    bytes16[] private s_subjects;

    function setCursed(bytes16 subject, bool cursed) external {
        if (!s_tracked[subject]) {
            s_tracked[subject] = true;
            s_subjects.push(subject);
        }
        s_cursed[subject] = cursed;
    }

    function getCursedSubjects() external view override returns (bytes16[] memory subjects) {
        uint256 cursedCount;
        for (uint256 i = 0; i < s_subjects.length; ++i) {
            if (s_cursed[s_subjects[i]]) {
                ++cursedCount;
            }
        }

        subjects = new bytes16[](cursedCount);
        uint256 outputIndex;
        for (uint256 i = 0; i < s_subjects.length; ++i) {
            bytes16 subject = s_subjects[i];
            if (s_cursed[subject]) {
                subjects[outputIndex++] = subject;
            }
        }
    }

    function isCursed() external view override returns (bool) {
        return s_cursed[bytes16(0)];
    }

    function isCursed(bytes16 subject) external view override returns (bool) {
        return s_cursed[bytes16(0)] || s_cursed[subject];
    }
}
