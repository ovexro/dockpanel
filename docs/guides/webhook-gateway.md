# Webhook Gateway Guide

The Webhook Gateway lets you receive, inspect, route, and replay incoming webhooks from external services like GitHub, Stripe, or any HTTP sender.

## Concepts

- **Endpoint**: A unique URL that receives incoming webhooks (e.g., `/api/webhooks/gateway/abc123…`)
- **Delivery**: Each received webhook is logged as a delivery with headers, body, and status
- **Route**: A rule that forwards matching deliveries to a target URL
- **HMAC Verification**: Optional signature verification to ensure webhooks are authentic

## Create an Endpoint

### From the Panel

1. Go to **Webhook Gateway** in the sidebar
2. Click **New Endpoint**
3. Configure:
   - **Name**: Descriptive label (e.g., "GitHub Deploys")
   - **HMAC Algorithm**: None, SHA-256 or SHA-1. Verification as a whole is optional.
   - **HMAC Secret**: The shared secret. Required once you pick an algorithm.
   - **HMAC Header**: The header containing the signature (e.g., `X-Hub-Signature-256`). Required once you pick an algorithm.
4. Click **Create**

An endpoint's verification settings cannot be edited afterwards — to change them,
delete the endpoint and create it again. That is why choosing an algorithm and
leaving the secret or the header empty is refused at creation rather than
accepted and repaired later.

You get a unique URL like `https://panel.example.com/api/webhooks/gateway/abc123…`,
shown on the Webhook Gateway page once the endpoint is selected. Copy it from
there and give it to the external service as their webhook destination. The path
carries the endpoint's token, so treat the URL itself as the secret.

## View Deliveries

1. Open an endpoint
2. The **Deliveries** tab shows every received webhook:
   - Timestamp
   - HTTP method and headers
   - Request body
   - Verification status (passed/failed/skipped)
   - Routing status (forwarded/filtered/failed)

Click any delivery to inspect the full request and response details.

## Create Routes

Routes forward incoming webhooks to target URLs based on optional JSON filters.

1. Open an endpoint
2. Go to the **Routes** tab
3. Click **Add Route**
4. Configure:
   - **Target URL**: Where to forward the webhook (e.g., `http://localhost:3000/deploy`)
   - **JSON Filter**: Optional JSONPath expression to match specific payloads
   - **Headers**: Optional extra headers to include in the forwarded request
5. Click **Save**

### JSON Filtering Examples

Forward only push events to the main branch:

```
$.ref == "refs/heads/main"
```

Forward only Stripe payment events:

```
$.type == "payment_intent.succeeded"
```

If no filter is set, all deliveries are forwarded.

## Pause an Endpoint or a Route

Both the endpoints table and the routes table carry a **Status** column and a
**Pause** / **Resume** button.

- Pausing an **endpoint** makes its public URL stop accepting deliveries. The
  sender gets a 404 as though the endpoint did not exist. Everything already
  recorded — deliveries, routes, counters — is kept.
- Pausing a **route** stops that one destination receiving forwarded deliveries,
  and leaves the rest of the endpoint's routes working.

Pause is the control to reach for when a sender misbehaves or a destination is
down. Deleting an endpoint also stops it, but it removes every delivery and
every route recorded against it — so it destroys the history you would need in
order to work out what went wrong.

## Replay Deliveries

If a delivery failed or you need to re-process it:

1. Find the delivery in the **Request Inspector** tab
2. Click **Replay** on its row
3. The delivery is re-sent to every enabled route whose filter matches it —
   the same routes the original delivery went to, and no others

This is useful for debugging or recovering from temporary downstream failures.

## HMAC Verification

When HMAC verification is configured:

1. DockPanel computes the HMAC signature of the request body using the shared secret
2. Compares it against the value in the configured header
3. Marks the delivery as **verified** or **failed**

Failed verifications are logged but not forwarded (unless you explicitly replay
them). Replaying one is a deliberate act — the panel asks you to confirm first,
because the payload was never vouched for and replaying it sends it onward with
each route's stored headers attached. The usual innocent cause is a sender
configured with the wrong secret; an endpoint's own secret cannot be edited
after creation, so re-verifying would reach the same verdict.

An endpoint that names an algorithm but holds no usable secret verifies nothing,
so every delivery to it is **rejected**, logged as failed, and not forwarded. The
endpoint list marks such an endpoint *no secret — rejecting*. Endpoints created
before v2.127.0 could reach that state; delete and recreate one to repair it.

The shared secret is never sent back to the browser. The endpoint list reports
only whether a secret is present.

### Provider-Specific Setup

| Provider | Header | Algorithm |
|----------|--------|-----------|
| GitHub | `X-Hub-Signature-256` | SHA-256 |
| Stripe | `Stripe-Signature` | SHA-256 (with timestamp) |
| GitLab | `X-Gitlab-Token` | Token comparison |
| Slack | `X-Slack-Signature` | SHA-256 |

## API Reference

See the [Webhook Gateway API](../api-reference.md#webhook-gateway) for all endpoints.
