---
sidebar_position: 2
---

# JWT Authentication

Advanced authentication using JSON Web Tokens for programmatic access.

## Overview

JWT authentication is useful for:
- Server-to-server communication
- Short-lived, scoped tokens
- Dynamic permission management
- Integration with existing auth systems

## Getting Started

JWT auth requires:
1. A PlexMCP organization
2. A service account (Enterprise)
3. Your organization's signing key

Contact sales@plexmcp.com to enable JWT authentication.

## How It Works

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│  Your Server    │────▶│   PlexMCP API    │────▶│   Your MCPs     │
│                 │     │                  │     │                 │
│  Generates JWT  │     │  Validates JWT   │     │  Processes      │
│  with claims    │     │  Extracts claims │     │  request        │
└─────────────────┘     └──────────────────┘     └─────────────────┘
```

1. Your server creates a JWT with specific claims
2. JWT is signed with your secret key
3. PlexMCP validates and extracts permissions
4. Request is processed if authorized

## JWT Structure

### Header

```json
{
  "alg": "HS256",
  "typ": "JWT"
}
```

### Payload Claims

| Claim | Required | Description |
|-------|----------|-------------|
| `iss` | Yes | Your organization ID |
| `sub` | Yes | Service account ID |
| `aud` | Yes | `api.plexmcp.com` |
| `iat` | Yes | Issued at (Unix timestamp) |
| `exp` | Yes | Expiration (Unix timestamp) |
| `mcps` | No | Allowed MCP IDs (array or `*`) |

### Example Payload

```json
{
  "iss": "org_123abc",
  "sub": "svc_xyz789",
  "aud": "api.plexmcp.com",
  "iat": 1705763400,
  "exp": 1705767000,
  "mcps": ["mcp_weather", "mcp_calculator"]
}
```

## Generating JWTs

### Node.js

```typescript
import jwt from 'jsonwebtoken';

function generatePlexMCPToken(options: {
  orgId: string;
  serviceAccountId: string;
  secretKey: string;
  mcps?: string[];
  expiresIn?: number;
}) {
  const now = Math.floor(Date.now() / 1000);

  const payload = {
    iss: options.orgId,
    sub: options.serviceAccountId,
    aud: 'api.plexmcp.com',
    iat: now,
    exp: now + (options.expiresIn || 3600),
    mcps: options.mcps || '*',
  };

  return jwt.sign(payload, options.secretKey, { algorithm: 'HS256' });
}

// Usage
const token = generatePlexMCPToken({
  orgId: 'org_123abc',
  serviceAccountId: 'svc_xyz789',
  secretKey: process.env.PLEXMCP_JWT_SECRET,
  mcps: ['mcp_weather'],
  expiresIn: 3600, // 1 hour
});
```

### Python

```python
import jwt
import time

def generate_plexmcp_token(
    org_id: str,
    service_account_id: str,
    secret_key: str,
    mcps: list = None,
    expires_in: int = 3600
) -> str:
    now = int(time.time())

    payload = {
        'iss': org_id,
        'sub': service_account_id,
        'aud': 'api.plexmcp.com',
        'iat': now,
        'exp': now + expires_in,
        'mcps': mcps or '*',
    }

    return jwt.encode(payload, secret_key, algorithm='HS256')

# Usage
token = generate_plexmcp_token(
    org_id='org_123abc',
    service_account_id='svc_xyz789',
    secret_key=os.environ['PLEXMCP_JWT_SECRET'],
    mcps=['mcp_weather'],
    expires_in=3600
)
```

## Using JWT Authentication

Include the JWT in the Authorization header:

```bash
curl -X POST https://api.plexmcp.com/mcp \
  -H "Authorization: Bearer eyJhbG..." \
  -H "Content-Type: application/json" \
  -d '{...}'
```

## Best Practices

### Short Expiration Times

Use short-lived tokens (1 hour or less):

```typescript
const token = generateToken({
  // ...
  expiresIn: 3600, // 1 hour max
});
```

### Minimal Scope

Only include necessary MCPs:

```typescript
const token = generateToken({
  // ...
  mcps: ['mcp_specific'], // Not '*'
});
```

### Rotate Secret Keys

Regularly rotate your JWT signing key:
1. Generate new key in dashboard
2. Update all services
3. Revoke old key

### Secure Key Storage

Store JWT secrets securely:
- AWS Secrets Manager
- HashiCorp Vault
- Environment variables (not code)

## Error Handling

### Common Errors

| Error | Cause | Fix |
|-------|-------|-----|
| `invalid_token` | Malformed JWT | Check token generation |
| `expired_token` | Token expired | Generate new token |
| `invalid_signature` | Wrong secret | Verify signing key |
| `invalid_audience` | Wrong `aud` claim | Use `api.plexmcp.com` |

### Error Response

```json
{
  "success": false,
  "error": {
    "code": "invalid_token",
    "message": "Token signature verification failed"
  }
}
```

## Comparison: API Keys vs JWT

| Feature | API Keys | JWT |
|---------|----------|-----|
| Setup | Simple | Complex |
| Expiration | Optional | Required |
| Dynamic scope | No | Yes |
| Revocation | Instant | Wait for expiry |
| Best for | Standard use | Enterprise |

## Enterprise Only

JWT authentication requires an Enterprise plan. Contact sales@plexmcp.com to enable.
