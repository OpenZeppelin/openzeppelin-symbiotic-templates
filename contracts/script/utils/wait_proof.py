#!/usr/bin/env python3
import sys
import json
import time
import urllib.request

# args: url message_id tries sleep_ms require_signed
url = sys.argv[1]
message_id = sys.argv[2]
tries = int(sys.argv[3]) if len(sys.argv) > 3 else 50
sleep_ms = int(sys.argv[4]) if len(sys.argv) > 4 else 200
require_signed = (sys.argv[5].lower() == "true") if len(sys.argv) > 5 else False

payload = {"message_ids": [message_id]}
body = json.dumps(payload).encode("utf-8")

for _ in range(tries):
    req = urllib.request.Request(url, data=body, method="POST")
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if message_id in data:
                proof = data[message_id]
                if require_signed:
                    root_proof = proof.get("root_proof", [])
                    if isinstance(root_proof, list) and len(root_proof) == 0:
                        pass
                    else:
                        print(json.dumps(proof))
                        sys.exit(0)
                else:
                    print(json.dumps(proof))
                    sys.exit(0)
    except Exception:
        pass
    time.sleep(sleep_ms / 1000.0)

print("{}")
sys.exit(0)
