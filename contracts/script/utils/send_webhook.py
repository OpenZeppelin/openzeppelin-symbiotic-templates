#!/usr/bin/env python3
import sys
import json
import time
import hmac
import hashlib
import urllib.request

# args: url secret chain_id block_number tx_hash log_index address data_hex topics_csv
url = sys.argv[1]
secret = sys.argv[2]
chain_id = int(sys.argv[3])
block_number = int(sys.argv[4])
tx_hash = sys.argv[5]
log_index = int(sys.argv[6])
address = sys.argv[7]
data_hex = sys.argv[8]
topics_csv = sys.argv[9]

if topics_csv:
    topics = topics_csv.split(",")
else:
    topics = []

payload = {
    "EVM": {
        "logs": [
            {
                "address": address,
                "topics": topics,
                "data": data_hex,
                "blockNumber": block_number,
                "transactionHash": tx_hash,
                "logIndex": log_index,
            }
        ],
        "matched_on_args": {
            "events": [
                {
                    "args": [],
                    "hex_signature": "",
                    "signature": "JobAssigned(bytes32,uint32,uint32,address,bytes32,bytes32,bytes,uint64,uint64,bytes,uint256)",
                }
            ]
        },
        "monitor": {"name": "Integration Test"},
        "network_slug": "local_anvil",
        "transaction": {
            "blockHash": "0x" + "00" * 32,
            "blockNumber": block_number,
            "transactionIndex": 0,
            "from": address,
            "to": None,
            "hash": tx_hash,
            "chainId": chain_id,
        },
    }
}

body = json.dumps(payload).encode("utf-8")

ts = str(int(time.time() * 1000))
mac = hmac.new(secret.encode("utf-8"), digestmod=hashlib.sha256)
mac.update(body + ts.encode("utf-8"))
signature = mac.hexdigest()

req = urllib.request.Request(url, data=body, method="POST")
req.add_header("Content-Type", "application/json")
req.add_header("X-Signature", signature)
req.add_header("X-Timestamp", ts)

try:
    with urllib.request.urlopen(req, timeout=5) as resp:
        print(resp.status)
        sys.exit(0 if resp.status == 200 else 1)
except Exception as e:
    print("error")
    sys.exit(1)
