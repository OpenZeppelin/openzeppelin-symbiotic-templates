#!/usr/bin/env python3
import sys
import json
import urllib.request

# args: url message_id
url = sys.argv[1]
message_id = sys.argv[2]

payload = {"message_ids": [message_id]}
body = json.dumps(payload).encode("utf-8")

req = urllib.request.Request(url, data=body, method="POST")
req.add_header("Content-Type", "application/json")

try:
    with urllib.request.urlopen(req, timeout=5) as resp:
        data = json.loads(resp.read().decode("utf-8"))
except Exception:
    print("{}")
    sys.exit(0)

if message_id in data:
    print(json.dumps(data[message_id]))
else:
    print("{}")
