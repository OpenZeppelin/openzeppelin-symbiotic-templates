// SPDX-License-Identifier: MIT
pragma solidity ^0.8.25;

/**
 * @title Elliptic curve operations on twist points for alt_bn128
 * @author Mustafa Al-Bassam (mus@musalbas.com)
 * @dev Homepage: https://github.com/musalbas/solidity-BN256G2
 * WARNING: this code is used ONLY for testing purposes, DO NOT USE IN PRODUCTION
 */
library BN254G2 {
    uint256 internal constant FIELD_MODULUS = 0x30644e72e131a029b85045b68181585d97816a916871ca8d3c208c16d87cfd47;
    uint256 internal constant TWISTBX = 0x2b149d40ceb8aaae81be18991be06ac3b5b4c5e559dbefa33267e6dc24a138e5;
    uint256 internal constant TWISTBY = 0x9713b03af0fed4cd2cafadeed8fdf4a74fa084e52d1852e4a2bd0685c315d2;
    uint256 internal constant PTXX = 0;
    uint256 internal constant PTXY = 1;
    uint256 internal constant PTYX = 2;
    uint256 internal constant PTYY = 3;
    uint256 internal constant PTZX = 4;
    uint256 internal constant PTZY = 5;

    function ECTwistMul(
        uint256 s,
        uint256 pt1xx,
        uint256 pt1xy,
        uint256 pt1yx,
        uint256 pt1yy
    ) internal view returns (uint256, uint256, uint256, uint256) {
        uint256 pt1zx = 1;
        if (pt1xx == 0 && pt1xy == 0 && pt1yx == 0 && pt1yy == 0) {
            pt1xx = 1;
            pt1yx = 1;
            pt1zx = 0;
        } else {
            assert(_isOnCurve(pt1xx, pt1xy, pt1yx, pt1yy));
        }
        uint256[6] memory pt2 = _ECTwistMulJacobian(s, pt1xx, pt1xy, pt1yx, pt1yy, pt1zx, 0);
        return _fromJacobian(pt2[PTXX], pt2[PTXY], pt2[PTYX], pt2[PTYY], pt2[PTZX], pt2[PTZY]);
    }

    function submod(uint256 a, uint256 b, uint256 n) internal pure returns (uint256) {
        return addmod(a, n - b, n);
    }

    function _FQ2Mul(uint256 xx, uint256 xy, uint256 yx, uint256 yy) internal pure returns (uint256, uint256) {
        return (
            submod(mulmod(xx, yx, FIELD_MODULUS), mulmod(xy, yy, FIELD_MODULUS), FIELD_MODULUS),
            addmod(mulmod(xx, yy, FIELD_MODULUS), mulmod(xy, yx, FIELD_MODULUS), FIELD_MODULUS)
        );
    }

    function _FQ2Muc(uint256 xx, uint256 xy, uint256 c) internal pure returns (uint256, uint256) {
        return (mulmod(xx, c, FIELD_MODULUS), mulmod(xy, c, FIELD_MODULUS));
    }

    function _FQ2Sub(uint256 xx, uint256 xy, uint256 yx, uint256 yy) internal pure returns (uint256, uint256) {
        return (submod(xx, yx, FIELD_MODULUS), submod(xy, yy, FIELD_MODULUS));
    }

    function _FQ2Inv(uint256 x, uint256 y) internal view returns (uint256, uint256) {
        uint256 inv = _modInv(
            addmod(mulmod(y, y, FIELD_MODULUS), mulmod(x, x, FIELD_MODULUS), FIELD_MODULUS),
            FIELD_MODULUS
        );
        return (mulmod(x, inv, FIELD_MODULUS), FIELD_MODULUS - mulmod(y, inv, FIELD_MODULUS));
    }

    function _isOnCurve(uint256 xx, uint256 xy, uint256 yx, uint256 yy) internal pure returns (bool) {
        uint256 yyx;
        uint256 yyy;
        uint256 xxxx;
        uint256 xxxy;
        (yyx, yyy) = _FQ2Mul(yx, yy, yx, yy);
        (xxxx, xxxy) = _FQ2Mul(xx, xy, xx, xy);
        (xxxx, xxxy) = _FQ2Mul(xxxx, xxxy, xx, xy);
        (yyx, yyy) = _FQ2Sub(yyx, yyy, xxxx, xxxy);
        (yyx, yyy) = _FQ2Sub(yyx, yyy, TWISTBX, TWISTBY);
        return yyx == 0 && yyy == 0;
    }

    function _modInv(uint256 a, uint256 n) internal view returns (uint256 result) {
        bool success;
        assembly ("memory-safe") {
            let freemem := mload(0x40)
            mstore(freemem, 0x20)
            mstore(add(freemem, 0x20), 0x20)
            mstore(add(freemem, 0x40), 0x20)
            mstore(add(freemem, 0x60), a)
            mstore(add(freemem, 0x80), sub(n, 2))
            mstore(add(freemem, 0xA0), n)
            success := staticcall(sub(gas(), 2000), 5, freemem, 0xC0, freemem, 0x20)
            result := mload(freemem)
        }
        require(success);
    }

    function _fromJacobian(
        uint256 pt1xx, uint256 pt1xy, uint256 pt1yx, uint256 pt1yy, uint256 pt1zx, uint256 pt1zy
    ) internal view returns (uint256 pt2xx, uint256 pt2xy, uint256 pt2yx, uint256 pt2yy) {
        uint256 invzx;
        uint256 invzy;
        (invzx, invzy) = _FQ2Inv(pt1zx, pt1zy);
        (pt2xx, pt2xy) = _FQ2Mul(pt1xx, pt1xy, invzx, invzy);
        (pt2yx, pt2yy) = _FQ2Mul(pt1yx, pt1yy, invzx, invzy);
    }

    struct _ECTwistAddJacobianArgs {
        uint256 pt1xx; uint256 pt1xy; uint256 pt1yx; uint256 pt1yy; uint256 pt1zx; uint256 pt1zy;
        uint256 pt2xx; uint256 pt2xy; uint256 pt2yx; uint256 pt2yy; uint256 pt2zx; uint256 pt2zy;
    }

    function _ECTwistAddJacobian(_ECTwistAddJacobianArgs memory $) internal pure returns (uint256[6] memory pt3) {
        if ($.pt1zx == 0 && $.pt1zy == 0) {
            (pt3[PTXX], pt3[PTXY], pt3[PTYX], pt3[PTYY], pt3[PTZX], pt3[PTZY]) =
                ($.pt2xx, $.pt2xy, $.pt2yx, $.pt2yy, $.pt2zx, $.pt2zy);
            return pt3;
        } else if ($.pt2zx == 0 && $.pt2zy == 0) {
            (pt3[PTXX], pt3[PTXY], pt3[PTYX], pt3[PTYY], pt3[PTZX], pt3[PTZY]) =
                ($.pt1xx, $.pt1xy, $.pt1yx, $.pt1yy, $.pt1zx, $.pt1zy);
            return pt3;
        }

        ($.pt2yx, $.pt2yy) = _FQ2Mul($.pt2yx, $.pt2yy, $.pt1zx, $.pt1zy);
        (pt3[PTYX], pt3[PTYY]) = _FQ2Mul($.pt1yx, $.pt1yy, $.pt2zx, $.pt2zy);
        ($.pt2xx, $.pt2xy) = _FQ2Mul($.pt2xx, $.pt2xy, $.pt1zx, $.pt1zy);
        (pt3[PTZX], pt3[PTZY]) = _FQ2Mul($.pt1xx, $.pt1xy, $.pt2zx, $.pt2zy);

        if ($.pt2xx == pt3[PTZX] && $.pt2xy == pt3[PTZY]) {
            if ($.pt2yx == pt3[PTYX] && $.pt2yy == pt3[PTYY]) {
                (pt3[PTXX], pt3[PTXY], pt3[PTYX], pt3[PTYY], pt3[PTZX], pt3[PTZY]) =
                    _ECTwistDoubleJacobian($.pt1xx, $.pt1xy, $.pt1yx, $.pt1yy, $.pt1zx, $.pt1zy);
                return pt3;
            }
            (pt3[PTXX], pt3[PTXY], pt3[PTYX], pt3[PTYY], pt3[PTZX], pt3[PTZY]) = (1, 0, 1, 0, 0, 0);
            return pt3;
        }

        ($.pt2zx, $.pt2zy) = _FQ2Mul($.pt1zx, $.pt1zy, $.pt2zx, $.pt2zy);
        ($.pt1xx, $.pt1xy) = _FQ2Sub($.pt2yx, $.pt2yy, pt3[PTYX], pt3[PTYY]);
        ($.pt1yx, $.pt1yy) = _FQ2Sub($.pt2xx, $.pt2xy, pt3[PTZX], pt3[PTZY]);
        ($.pt1zx, $.pt1zy) = _FQ2Mul($.pt1yx, $.pt1yy, $.pt1yx, $.pt1yy);
        ($.pt2yx, $.pt2yy) = _FQ2Mul($.pt1zx, $.pt1zy, pt3[PTZX], pt3[PTZY]);
        ($.pt1zx, $.pt1zy) = _FQ2Mul($.pt1zx, $.pt1zy, $.pt1yx, $.pt1yy);
        (pt3[PTZX], pt3[PTZY]) = _FQ2Mul($.pt1zx, $.pt1zy, $.pt2zx, $.pt2zy);
        ($.pt2xx, $.pt2xy) = _FQ2Mul($.pt1xx, $.pt1xy, $.pt1xx, $.pt1xy);
        ($.pt2xx, $.pt2xy) = _FQ2Mul($.pt2xx, $.pt2xy, $.pt2zx, $.pt2zy);
        ($.pt2xx, $.pt2xy) = _FQ2Sub($.pt2xx, $.pt2xy, $.pt1zx, $.pt1zy);
        ($.pt2zx, $.pt2zy) = _FQ2Muc($.pt2yx, $.pt2yy, 2);
        ($.pt2xx, $.pt2xy) = _FQ2Sub($.pt2xx, $.pt2xy, $.pt2zx, $.pt2zy);
        (pt3[PTXX], pt3[PTXY]) = _FQ2Mul($.pt1yx, $.pt1yy, $.pt2xx, $.pt2xy);
        ($.pt1yx, $.pt1yy) = _FQ2Sub($.pt2yx, $.pt2yy, $.pt2xx, $.pt2xy);
        ($.pt1yx, $.pt1yy) = _FQ2Mul($.pt1xx, $.pt1xy, $.pt1yx, $.pt1yy);
        ($.pt1xx, $.pt1xy) = _FQ2Mul($.pt1zx, $.pt1zy, pt3[PTYX], pt3[PTYY]);
        (pt3[PTYX], pt3[PTYY]) = _FQ2Sub($.pt1yx, $.pt1yy, $.pt1xx, $.pt1xy);
    }

    function _ECTwistDoubleJacobian(
        uint256 pt1xx, uint256 pt1xy, uint256 pt1yx, uint256 pt1yy, uint256 pt1zx, uint256 pt1zy
    ) internal pure returns (uint256 pt2xx, uint256 pt2xy, uint256 pt2yx, uint256 pt2yy, uint256 pt2zx, uint256 pt2zy) {
        (pt2xx, pt2xy) = _FQ2Muc(pt1xx, pt1xy, 3);
        (pt2xx, pt2xy) = _FQ2Mul(pt2xx, pt2xy, pt1xx, pt1xy);
        (pt1zx, pt1zy) = _FQ2Mul(pt1yx, pt1yy, pt1zx, pt1zy);
        (pt2yx, pt2yy) = _FQ2Mul(pt1xx, pt1xy, pt1yx, pt1yy);
        (pt2yx, pt2yy) = _FQ2Mul(pt2yx, pt2yy, pt1zx, pt1zy);
        (pt1xx, pt1xy) = _FQ2Mul(pt2xx, pt2xy, pt2xx, pt2xy);
        (pt2zx, pt2zy) = _FQ2Muc(pt2yx, pt2yy, 8);
        (pt1xx, pt1xy) = _FQ2Sub(pt1xx, pt1xy, pt2zx, pt2zy);
        (pt2zx, pt2zy) = _FQ2Mul(pt1zx, pt1zy, pt1zx, pt1zy);
        (pt2yx, pt2yy) = _FQ2Muc(pt2yx, pt2yy, 4);
        (pt2yx, pt2yy) = _FQ2Sub(pt2yx, pt2yy, pt1xx, pt1xy);
        (pt2yx, pt2yy) = _FQ2Mul(pt2yx, pt2yy, pt2xx, pt2xy);
        (pt2xx, pt2xy) = _FQ2Muc(pt1yx, pt1yy, 8);
        (pt2xx, pt2xy) = _FQ2Mul(pt2xx, pt2xy, pt1yx, pt1yy);
        (pt2xx, pt2xy) = _FQ2Mul(pt2xx, pt2xy, pt2zx, pt2zy);
        (pt2yx, pt2yy) = _FQ2Sub(pt2yx, pt2yy, pt2xx, pt2xy);
        (pt2xx, pt2xy) = _FQ2Muc(pt1xx, pt1xy, 2);
        (pt2xx, pt2xy) = _FQ2Mul(pt2xx, pt2xy, pt1zx, pt1zy);
        (pt2zx, pt2zy) = _FQ2Mul(pt1zx, pt1zy, pt2zx, pt2zy);
        (pt2zx, pt2zy) = _FQ2Muc(pt2zx, pt2zy, 8);
    }

    function _ECTwistMulJacobian(
        uint256 d, uint256 pt1xx, uint256 pt1xy, uint256 pt1yx, uint256 pt1yy, uint256 pt1zx, uint256 pt1zy
    ) internal pure returns (uint256[6] memory pt2) {
        while (d != 0) {
            if ((d & 1) != 0) {
                pt2 = _ECTwistAddJacobian(_ECTwistAddJacobianArgs(
                    pt2[PTXX], pt2[PTXY], pt2[PTYX], pt2[PTYY], pt2[PTZX], pt2[PTZY],
                    pt1xx, pt1xy, pt1yx, pt1yy, pt1zx, pt1zy
                ));
            }
            (pt1xx, pt1xy, pt1yx, pt1yy, pt1zx, pt1zy) =
                _ECTwistDoubleJacobian(pt1xx, pt1xy, pt1yx, pt1yy, pt1zx, pt1zy);
            d = d / 2;
        }
    }
}
