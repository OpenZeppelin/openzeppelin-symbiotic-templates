#!/usr/bin/env python3
"""
Multi-operator webhook fanout script for oz-monitor.

Sends the same event to all configured operator endpoints (WEBHOOK_URL_1, WEBHOOK_URL_2, WEBHOOK_URL_3).
Each operator independently processes the event, signs via its paired symbiotic-relay,
and submits proofs to oz-relayer. On-chain deduplication handles duplicate submissions.
"""
import sys
import json
import os
from urllib.request import Request, urlopen
from urllib.error import URLError, HTTPError
import hashlib
import hmac
import time


def get_webhook_urls():
    """Collect all WEBHOOK_URL_N environment variables."""
    urls = []
    for i in range(1, 10):  # Support up to 9 operators
        url = os.environ.get(f"WEBHOOK_URL_{i}")
        if url:
            urls.append(url)

    # Fallback to single WEBHOOK_URL for backwards compatibility
    if not urls:
        single_url = os.environ.get("WEBHOOK_URL")
        if single_url:
            urls.append(single_url)

    return urls


def send_to_operator(url, payload_bytes, headers):
    """Send webhook to a single operator endpoint."""
    request = Request(url, data=payload_bytes, headers=headers, method='POST')

    try:
        with urlopen(request, timeout=5) as response:
            status_code = response.getcode()
            response_body = response.read().decode('utf-8')
            print(f"  [{url}] OK ({status_code})")
            return True
    except HTTPError as e:
        print(f"  [{url}] HTTP Error {e.code}: {e.reason}", file=sys.stderr)
        return False
    except URLError as e:
        print(f"  [{url}] URL Error: {e.reason}", file=sys.stderr)
        return False
    except Exception as e:
        print(f"  [{url}] Unexpected error: {e}", file=sys.stderr)
        return False


def send_webhook():
    # Read monitor match data from stdin
    try:
        data = json.load(sys.stdin)
    except json.JSONDecodeError as e:
        print(f"Error parsing JSON input: {e}", file=sys.stderr)
        sys.exit(1)

    # Get webhook configuration
    webhook_urls = get_webhook_urls()
    webhook_secret = os.environ.get("WEBHOOK_SECRET")

    if not webhook_urls:
        print("Error: No WEBHOOK_URL environment variables set", file=sys.stderr)
        sys.exit(1)

    # Convert payload to JSON bytes
    payload_bytes = json.dumps(data).encode('utf-8')

    # Get monitor name for event type header
    monitor_name = data.get('monitor_match', {}).get('EVM', {}).get('monitor', {}).get('name', 'unknown')

    # Prepare headers
    headers = {
        "Content-Type": "application/json",
        "Content-Length": str(len(payload_bytes)),
        "X-Event-Type": monitor_name
    }

    # Add HMAC signature if secret is provided
    if webhook_secret:
        timestamp_ms = str(int(time.time() * 1000))
        message = payload_bytes + timestamp_ms.encode('utf-8')
        signature = hmac.new(
            webhook_secret.encode('utf-8'),
            message,
            hashlib.sha256
        ).hexdigest()
        headers["X-Signature"] = signature
        headers["X-Timestamp"] = timestamp_ms

    # Fan out to all operators
    print(f"Sending webhook to {len(webhook_urls)} operator(s)...")
    success_count = 0

    for url in webhook_urls:
        if send_to_operator(url, payload_bytes, headers):
            success_count += 1

    print(f"Webhook fanout complete: {success_count}/{len(webhook_urls)} succeeded")

    # Exit success if at least one operator received the event
    if success_count > 0:
        sys.exit(0)
    else:
        print("Error: All webhook deliveries failed", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    send_webhook()
