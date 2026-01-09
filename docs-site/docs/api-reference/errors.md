---
sidebar_position: 6
---

# Error Handling

Understanding PlexMCP API errors and how to handle them.

## Error Response Format

All errors follow a consistent format:

```json
{
  "success": false,
  "error": {
    "code": "error_code",
    "message": "Human-readable message",
    "details": {
      // Additional context (optional)
    }
  }
}
```

## HTTP Status Codes

| Code | Name | Description |
|------|------|-------------|
| 400 | Bad Request | Invalid request parameters |
| 401 | Unauthorized | Missing or invalid authentication |
| 403 | Forbidden | Valid auth but insufficient permissions |
| 404 | Not Found | Resource doesn't exist |
| 409 | Conflict | Request conflicts with current state |
| 422 | Unprocessable Entity | Validation failed |
| 429 | Too Many Requests | Rate limit exceeded |
| 500 | Internal Server Error | Server error |
| 502 | Bad Gateway | MCP server unreachable |
| 503 | Service Unavailable | PlexMCP is temporarily down |
| 504 | Gateway Timeout | MCP server timeout |

## Error Codes

### Authentication Errors

#### `unauthorized`
```json
{
  "code": "unauthorized",
  "message": "Invalid or missing API key"
}
```
**Cause**: Missing Authorization header or invalid key
**Fix**: Include valid API key in Authorization header

#### `expired_key`
```json
{
  "code": "expired_key",
  "message": "API key has expired"
}
```
**Cause**: Key passed its expiration date
**Fix**: Create a new API key

#### `revoked_key`
```json
{
  "code": "revoked_key",
  "message": "API key has been revoked"
}
```
**Cause**: Key was manually revoked
**Fix**: Create a new API key

### Permission Errors

#### `forbidden`
```json
{
  "code": "forbidden",
  "message": "You don't have permission to perform this action"
}
```
**Cause**: User/key lacks required permissions
**Fix**: Check role permissions or key MCP access

#### `mcp_not_permitted`
```json
{
  "code": "mcp_not_permitted",
  "message": "API key does not have access to this MCP"
}
```
**Cause**: Key permissions don't include requested MCP
**Fix**: Create key with access to this MCP

### Resource Errors

#### `not_found`
```json
{
  "code": "not_found",
  "message": "Resource not found"
}
```
**Cause**: Requested resource doesn't exist
**Fix**: Verify resource ID is correct

#### `already_exists`
```json
{
  "code": "already_exists",
  "message": "Resource already exists"
}
```
**Cause**: Trying to create duplicate resource
**Fix**: Use unique identifiers

### Validation Errors

#### `validation_error`
```json
{
  "code": "validation_error",
  "message": "Validation failed",
  "details": {
    "field_name": "Error description"
  }
}
```
**Cause**: Request body failed validation
**Fix**: Check the details field for specific issues

#### `invalid_request`
```json
{
  "code": "invalid_request",
  "message": "Invalid request format"
}
```
**Cause**: Malformed JSON or wrong content type
**Fix**: Ensure valid JSON with Content-Type: application/json

### Rate Limit Errors

#### `rate_limited`
```json
{
  "code": "rate_limited",
  "message": "Too many requests"
}
```
**Cause**: Exceeded requests per second limit (PlexMCP Cloud only)
**Fix**: Implement retry with exponential backoff

The `Retry-After` header indicates seconds to wait before retrying.

### Usage Limit Errors

#### `quota_exceeded`
```json
{
  "code": "quota_exceeded",
  "message": "Monthly request quota exceeded"
}
```
**Cause**: Hit monthly request limit (Free plan)
**Fix**: Upgrade plan or wait for next billing period

### MCP Errors

#### `mcp_unreachable`
```json
{
  "code": "mcp_unreachable",
  "message": "Could not connect to MCP server"
}
```
**Cause**: MCP server not responding
**Fix**: Check MCP server status

#### `mcp_timeout`
```json
{
  "code": "mcp_timeout",
  "message": "MCP server did not respond in time"
}
```
**Cause**: MCP took too long to respond
**Fix**: Retry or check MCP performance

#### `mcp_error`
```json
{
  "code": "mcp_error",
  "message": "MCP returned an error",
  "details": {
    "mcp_error": "Tool not found: unknown_tool"
  }
}
```
**Cause**: MCP server returned an error
**Fix**: Check the details for MCP-specific error

### Server Errors

#### `internal_error`
```json
{
  "code": "internal_error",
  "message": "An unexpected error occurred"
}
```
**Cause**: PlexMCP server error
**Fix**: Retry request; contact support if persistent

## Handling Errors

### TypeScript/JavaScript

```typescript
try {
  const result = await client.mcp.invoke({
    mcpId: 'mcp_123',
    tool: 'get_weather',
    arguments: { location: 'SF' }
  });
} catch (error) {
  if (error.code === 'rate_limited') {
    const retryAfter = error.headers['retry-after'];
    await sleep(retryAfter * 1000);
    // Retry request
  } else if (error.code === 'mcp_unreachable') {
    console.error('MCP is down, using fallback');
    // Use fallback logic
  } else {
    throw error;
  }
}
```

### Python

```python
from plexmcp import PlexMCPError

try:
    result = client.mcp.invoke(
        mcp_id="mcp_123",
        tool="get_weather",
        arguments={"location": "SF"}
    )
except PlexMCPError as e:
    if e.code == "rate_limited":
        time.sleep(e.retry_after)
        # Retry request
    elif e.code == "mcp_unreachable":
        print("MCP is down, using fallback")
        # Use fallback logic
    else:
        raise
```

### Retry Strategy

Recommended exponential backoff:

```typescript
async function retryWithBackoff(fn, maxRetries = 3) {
  for (let i = 0; i < maxRetries; i++) {
    try {
      return await fn();
    } catch (error) {
      if (error.code === 'rate_limited') {
        const baseDelay = error.headers['retry-after'] || 1;
        const delay = baseDelay * Math.pow(2, i);
        await sleep(delay * 1000);
      } else if (error.code === 'internal_error' && i < maxRetries - 1) {
        const delay = Math.pow(2, i) * 1000;
        await sleep(delay);
      } else {
        throw error;
      }
    }
  }
}
```

## Debugging Tips

1. **Check the error code**: Specific codes help narrow down issues
2. **Review details field**: Contains field-specific validation errors
3. **Check headers**: Rate limit headers show remaining quota
4. **Enable logging**: Log full responses during development
5. **Use separate keys**: Use dedicated API keys for development environments
