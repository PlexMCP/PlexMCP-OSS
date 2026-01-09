---
sidebar_position: 1
---

# Registering MCPs

Learn how to add your MCP servers to PlexMCP for centralized access and management.

## What is an MCP?

Model Context Protocol (MCP) servers are tools that AI agents can interact with. They expose:
- **Tools**: Functions the AI can call
- **Resources**: Data the AI can read
- **Prompts**: Templates for common tasks

PlexMCP acts as a gateway, allowing you to manage multiple MCPs through a single interface.

## Registering via Dashboard

### Step 1: Access the MCPs Page

1. Log in to [dashboard.plexmcp.com](https://dashboard.plexmcp.com)
2. Click **MCPs** in the sidebar
3. Click **Add MCP**

### Step 2: Basic Information

| Field | Required | Description |
|-------|----------|-------------|
| **Name** | Yes | Display name for your MCP |
| **Endpoint URL** | Yes | URL where your MCP is hosted |
| **Description** | No | Notes about this MCP |

### Step 3: Authentication (Optional)

If your MCP requires authentication:

| Auth Type | Description |
|-----------|-------------|
| **None** | No authentication required |
| **Bearer Token** | Token in Authorization header |
| **API Key** | Key in custom header |
| **Basic Auth** | Username and password |

### Step 4: Test Connection

Click **Test Connection** to verify:
- Endpoint is reachable
- Authentication works
- MCP responds correctly

### Step 5: Create

Click **Create MCP** to add it to your organization.

## Registering via API

### Create MCP Request

```bash
curl -X POST "https://api.plexmcp.com/v1/mcps" \
  -H "Authorization: ApiKey $PLEXMCP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Weather API",
    "endpoint_url": "https://weather-mcp.example.com",
    "description": "Provides weather data and forecasts",
    "auth_type": "bearer",
    "auth_token": "mcp_secret_token"
  }'
```

### Response

```json
{
  "success": true,
  "data": {
    "id": "mcp_abc123",
    "name": "Weather API",
    "endpoint_url": "https://weather-mcp.example.com",
    "status": "active",
    "health": "unknown",
    "created_at": "2024-01-20T10:00:00Z"
  }
}
```

## MCP Requirements

### Endpoint URL

Your MCP endpoint must:
- Be accessible from the internet
- Use HTTPS (required for production)
- Respond within 30 seconds
- Implement MCP protocol correctly

### Protocol Compatibility

PlexMCP supports:
- **MCP 2024-11-05**: Current protocol version
- **HTTP transport**: Standard REST-like communication
- **SSE transport**: Server-Sent Events for streaming
- **WebSocket**: Real-time bidirectional (Enterprise)

### Health Checks

PlexMCP monitors your MCPs:
- Health check every 60 seconds
- 30-second timeout per check
- 3 retries before marking unhealthy

## Authentication Options

### No Authentication

For public MCPs:

```json
{
  "auth_type": "none"
}
```

### Bearer Token

Standard token authentication:

```json
{
  "auth_type": "bearer",
  "auth_token": "your_secret_token"
}
```

Sent as: `Authorization: Bearer your_secret_token`

### API Key Header

Custom header for API key:

```json
{
  "auth_type": "api_key",
  "auth_header": "X-API-Key",
  "auth_token": "your_api_key"
}
```

Sent as: `X-API-Key: your_api_key`

### Basic Authentication

Username and password:

```json
{
  "auth_type": "basic",
  "auth_username": "user",
  "auth_password": "password"
}
```

Sent as: `Authorization: Basic base64(user:password)`

## Testing Your MCP

Before registering, test your MCP directly:

```bash
# List tools
curl -X POST "https://your-mcp.example.com/tools/list" \
  -H "Content-Type: application/json" \
  -d '{}'

# Call a tool
curl -X POST "https://your-mcp.example.com/tools/call" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "get_weather",
    "arguments": {"location": "SF"}
  }'
```

## Best Practices

### 1. Use HTTPS

Always use HTTPS for production MCPs.

### 2. Descriptive Names

Choose names that help your team:
- Good: "Weather API - Production"
- Bad: "MCP 1"

### 3. Add Descriptions

Document what each MCP does and its available tools.

### 4. Secure Authentication

- Use strong, random tokens
- Rotate credentials regularly
- Store secrets securely

### 5. Monitor Health

Check the dashboard for health status:
- Green: Healthy
- Red: Unhealthy
- Gray: Unknown

## Troubleshooting

### Connection Test Fails

1. **Verify URL**: Check for typos
2. **Check access**: Is the MCP accessible from internet?
3. **Test directly**: Try curl from your machine
4. **Check auth**: Verify credentials are correct

### MCP Shows Unhealthy

1. **Check MCP logs**: Look for errors
2. **Test endpoint**: Verify it's responding
3. **Check timeout**: MCPs must respond in 30s
4. **Review protocol**: Ensure correct MCP implementation

### Tools Not Appearing

1. **Check tools/list endpoint**: Test directly
2. **Verify authentication**: Some MCPs require auth for listing
3. **Wait for sync**: New MCPs may take a minute to sync
