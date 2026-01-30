#!/usr/bin/env python3
import sys
import time
import urllib.request

url = sys.argv[1]
tries = int(sys.argv[2]) if len(sys.argv) > 2 else 30
sleep_ms = int(sys.argv[3]) if len(sys.argv) > 3 else 200

for _ in range(tries):
    try:
        with urllib.request.urlopen(url, timeout=2) as resp:
            if resp.status == 200:
                print("ok")
                sys.exit(0)
    except Exception:
        pass
    time.sleep(sleep_ms / 1000.0)

print("fail")
sys.exit(1)
