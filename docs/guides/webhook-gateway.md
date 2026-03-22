# Webhook Gateway Guide

The Webhook Gateway lets you receive, inspect, route, and replay incoming webhooks from external services like GitHub, Stripe, or any HTTP sender.

## Concepts

- **Endpoint**: A unique URL that receives incoming webhooks (e.g., `/hook/abc123`)
- **Delivery**: Each received webhook is logged as a delivery with headers, body, and status
- **Route**: A rule that forwards matching deliveries to a target URL
- **HMAC Verification**: Optional signature verification to ensure webhooks are authentic

## Create an Endpoint

### From the Panel

1. Go to **Webhook Gateway** in the sidebar
2. Click **New Endpoint**
3. Configure:
   - **Name**: Descriptive label (e.g., "GitHub Deploys")
   - **HMAC Secret**: Optional shared secret for signature verification
   - **HMAC Header**: The header containing the signature (e.g., `X-Hub-Signature-256`)
   - **HMAC Algorithm**: SHA-256, SHA-1, etc.
4. Click **Create**

You get a unique URL like `https://panel.example.com/hook/abc123`. Give this URL to the external service as their webhook destination.

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

## Replay Deliveries

If a delivery failed or you need to re-process it:

1. Open the delivery
2. Click **Replay**
3. The delivery is re-sent to all matching routes

This is useful for debugging or recovering from temporary downstream failures.

## HMAC Verification

When HMAC verification is configured:

1. DockPanel computes the HMAC signature of the request body using the shared secret
2. Compares it against the value in the configured header
3. Marks the delivery as **verified** or **failed**

Failed verifications are logged but not forwarded (unless you explicitly replay them).

### Provider-Specific Setup

| Provider | Header | Algorithm |
|----------|--------|-----------|
| GitHub | `X-Hub-Signature-256` | SHA-256 |
| Stripe | `Stripe-Signature` | SHA-256 (with timestamp) |
| GitLab | `X-Gitlab-Token` | Token comparison |
| Slack | `X-Slack-Signature` | SHA-256 |

## API Reference

See the [Webhook Gateway API](../api-reference.md#webhook-gateway) for all endpoints.
