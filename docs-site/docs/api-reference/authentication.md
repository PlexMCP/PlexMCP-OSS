---
sidebar_position: 2
---

# Authentication

All PlexMCP API requests require authentication. This guide covers how to authenticate your requests.

## API Keys

The primary authentication method is via API keys. Each key:
- Is tied to your organization
- Has specific permissions
- Can be revoked instantly
- Tracks usage independently

## Using Your API Key

Include your API key in the `Authorization` header:

```bash
curl -X GET https://api.plexmcp.com/v1/mcps \
  -H "Authorization: ApiKey YOUR_API_KEY"
```

### Header Format

```
Authorization: ApiKey pmcp_xxxxxxxxxxxxxxxxxxxxx
```

- **Prefix**: Always `ApiKey` (with space) for API keys
- **Key format**: Starts with `pmcp_` prefix

## Key Types

All API keys use the `pmcp_` prefix and work across all environments:

- Count toward usage limits
- Use in production applications
- Never expose in client-side code
- Separate keys recommended for development vs production

## Getting Your API Key

1. Log in to [dashboard.plexmcp.com](https://dashboard.plexmcp.com)
2. Navigate to **API Keys**
3. Click **Create API Key**
4. Configure name, expiration, and permissions
5. Copy the key immediately (shown only once)

## Key Permissions

Each key can be scoped to specific MCPs:

### All MCPs

```json
{
  "permissions": {
    "mcps": "*"
  }
}
```

Access all current and future MCPs.

### Specific MCPs

```json
{
  "permissions": {
    "mcps": ["mcp_123", "mcp_456"]
  }
}
```

Access only listed MCPs.

## Authentication Errors

### 401 Unauthorized

```json
{
  "success": false,
  "error": {
    "code": "unauthorized",
    "message": "Invalid or missing API key"
  }
}
```

Causes:
- Missing Authorization header
- Invalid API key format
- Revoked API key
- Expired API key

### 403 Forbidden

```json
{
  "success": false,
  "error": {
    "code": "forbidden",
    "message": "API key does not have permission for this resource"
  }
}
```

Causes:
- Key doesn't have access to requested MCP
- Key is test mode but accessing live resources
- Organization-level restriction

## Security Best Practices

### Never Expose Keys in Client Code

```javascript
// BAD - Don't do this!
const apiKey = "pmcp_xxxxx";

// GOOD - Use environment variables
const apiKey = process.env.PLEXMCP_API_KEY;
```

### Use Environment Variables

```bash
# .env file (not committed to git)
PLEXMCP_API_KEY=pmcp_xxxxx

# In your code
const apiKey = process.env.PLEXMCP_API_KEY;
```

### Rotate Keys Regularly

1. Create a new key
2. Update your applications
3. Verify everything works
4. Revoke the old key

### Use Minimal Permissions

Only grant access to MCPs the key actually needs.

### Set Expiration Dates

For temporary access or contractors:
- Short-term: 30 days
- Regular rotation: 90 days
- Long-term: 1 year max

### Monitor Key Usage

Check the dashboard for:
- Unusual request patterns
- Unexpected locations
- Error rate spikes

## Code Examples

### Node.js / TypeScript

```typescript
import { PlexMCP } from '@plexmcp/sdk';

const client = new PlexMCP({
  apiKey: process.env.PLEXMCP_API_KEY,
});
```

### Python

```python
import os
from plexmcp import PlexMCP

client = PlexMCP(api_key=os.environ["PLEXMCP_API_KEY"])
```

### cURL

```bash
export PLEXMCP_API_KEY="pmcp_xxxxx"

curl -X GET https://api.plexmcp.com/v1/mcps \
  -H "Authorization: ApiKey $PLEXMCP_API_KEY"
```

### Go

```go
package main

import (
    "os"
    "github.com/PlexMCP/PlexMCP-OSS-go"
)

func main() {
    client := plexmcp.NewClient(os.Getenv("PLEXMCP_API_KEY"))
}
```

## Troubleshooting

### "Invalid API Key" Error

1. Verify the complete key was copied
2. Check for trailing whitespace
3. Ensure `ApiKey ` prefix is present
4. Verify the key hasn't been revoked

### Key Works in cURL but Not in Code

1. Check environment variable is set
2. Verify no extra characters in key
3. Ensure proper header formatting
4. Check for HTTPS requirement

### Key Suddenly Stopped Working

1. Check if key was revoked
2. Verify key hasn't expired
3. Check usage limits
4. Review recent activity for security issues
