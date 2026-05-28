import base64
import binascii
import hashlib
import hmac
import os
from datetime import datetime, timedelta, timezone
from typing import Any

from fastapi import FastAPI, Header, HTTPException, Request


app = FastAPI()


def verify_signature(secret: str, body: bytes, signature: str | None) -> None:
    if not signature or not signature.startswith("sha256="):
        raise HTTPException(status_code=401, detail="invalid signature")

    expected = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    if not hmac.compare_digest(signature, expected):
        raise HTTPException(status_code=401, detail="invalid signature")


@app.post("/")
async def evaluate(
    request: Request,
    x_hook_signature: str | None = Header(default=None),
) -> dict[str, Any]:
    secret = os.environ.get("HOOK_SECRET")
    if not secret:
        raise HTTPException(status_code=500, detail="HOOK_SECRET not configured")

    body = await request.body()
    verify_signature(secret, body, x_hook_signature)

    try:
        payload = await request.json()
        message = payload["message"]
        context = payload["context"]
        if not isinstance(message, dict) or not isinstance(context, dict):
            raise TypeError
        defer_count = context["defer_count"]
        if not isinstance(defer_count, int):
            raise TypeError
        # Decode the provider-specific payload when policy needs to inspect it.
        raw_payload = base64.b64decode(message["data"], validate=True)
    except (binascii.Error, KeyError, TypeError, ValueError):
        raise HTTPException(status_code=400, detail="invalid request payload") from None

    if defer_count >= 5:
        return {"decision": "reject", "reason": "approval service gave up after 5 defers"}

    if raw_payload == b"reject":
        return {"decision": "reject", "reason": "payload requested rejection"}

    if raw_payload == b"defer":
        until = datetime.now(timezone.utc) + timedelta(seconds=30)
        return {
            "decision": "defer",
            "until": until.isoformat().replace("+00:00", "Z"),
            "reason": "awaiting approval",
        }

    return {"decision": "accept"}
