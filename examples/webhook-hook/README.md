# Webhook Acceptance Hook

Minimal FastAPI reference for the operator acceptance-hook webhook wire format.

This example is intentionally written outside Rust. Native hooks are the Rust extension point; webhook hooks are external services and can be implemented in any language that can receive HTTP, verify HMAC-SHA256, and return the decision JSON.

Run:

```bash
export HOOK_SECRET="shared-secret"
uvicorn main:app --host 0.0.0.0 --port 8088
```

Configure an operator hook:

```json
{
  "type": "webhook",
  "name": "approval",
  "url": "http://localhost:8088/",
  "secret": "shared-secret"
}
```
