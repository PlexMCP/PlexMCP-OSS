---
sidebar_position: 2
---

# Invoking MCP Tools

Learn how to call tools on your MCPs through the PlexMCP gateway.

## Overview

PlexMCP routes tool invocations to your MCPs:

```
Your App → PlexMCP Gateway → Your MCP → Response → Your App
```

Benefits:
- Single API key for multiple MCPs
- Automatic load balancing
- Usage tracking and analytics
- Unified error handling

## Basic Invocation

### API Request

```bash
curl -X POST "https://api.plexmcp.com/mcp" \
  -H "Authorization: ApiKey $PLEXMCP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "mcp_id": "mcp_weather123",
    "tool": "get_weather",
    "arguments": {
      "location": "San Francisco, CA"
    }
  }'
```

See [MCP API Reference](/api-reference/mcps#invoke-mcp-tool) for full request/response format.

### Response

```json
{
  "success": true,
  "data": {
    "result": {
      "location": "San Francisco, CA",
      "temperature": 65,
      "conditions": "Partly cloudy"
    },
    "metadata": {
      "mcp_id": "mcp_weather123",
      "tool": "get_weather",
      "latency_ms": 125
    }
  }
}
```

## Request Format

### Required Fields

| Field | Type | Description |
|-------|------|-------------|
| `mcp_id` | string | The MCP to call |
| `tool` | string | Tool name to invoke |
| `arguments` | object | Tool-specific arguments |

### Optional Fields

| Field | Type | Description |
|-------|------|-------------|
| `timeout` | integer | Max wait time (ms) |
| `context` | object | Additional context |

## SDK Examples

### TypeScript

```typescript
import { PlexMCP } from '@plexmcp/sdk';

const client = new PlexMCP({
  apiKey: process.env.PLEXMCP_API_KEY,
});

const result = await client.mcp.invoke({
  mcpId: 'mcp_weather123',
  tool: 'get_weather',
  arguments: {
    location: 'San Francisco',
    units: 'fahrenheit',
  },
});

console.log(result.temperature);
```

### Python

```python
from plexmcp import PlexMCP

client = PlexMCP(api_key=os.environ["PLEXMCP_API_KEY"])

result = client.mcp.invoke(
    mcp_id="mcp_weather123",
    tool="get_weather",
    arguments={
        "location": "San Francisco",
        "units": "fahrenheit"
    }
)

print(result["temperature"])
```

## Discovering Tools

### List Available Tools

Get tools for a specific MCP:

```bash
curl -X GET "https://api.plexmcp.com/v1/mcps/mcp_weather123" \
  -H "Authorization: ApiKey $PLEXMCP_API_KEY"
```

Response includes tool definitions:

```json
{
  "success": true,
  "data": {
    "id": "mcp_weather123",
    "name": "Weather API",
    "tools": [
      {
        "name": "get_weather",
        "description": "Get current weather for a location",
        "input_schema": {
          "type": "object",
          "properties": {
            "location": {
              "type": "string",
              "description": "City name or coordinates"
            },
            "units": {
              "type": "string",
              "enum": ["celsius", "fahrenheit"],
              "default": "fahrenheit"
            }
          },
          "required": ["location"]
        }
      }
    ]
  }
}
```

### Via Dashboard

1. Go to **MCPs**
2. Click on an MCP
3. View the **Tools** tab

## Error Handling

### Common Errors

| Error Code | Description | Solution |
|------------|-------------|----------|
| `mcp_unreachable` | MCP not responding | Check MCP status |
| `tool_not_found` | Tool doesn't exist | Verify tool name |
| `invalid_arguments` | Wrong argument format | Check tool schema |
| `mcp_timeout` | Request took too long | Increase timeout |

### Error Response

```json
{
  "success": false,
  "error": {
    "code": "tool_not_found",
    "message": "Tool 'get_whether' not found. Did you mean 'get_weather'?"
  }
}
```

### Handling in Code

```typescript
try {
  const result = await client.mcp.invoke({
    mcpId: 'mcp_weather',
    tool: 'get_weather',
    arguments: { location: 'SF' },
  });
} catch (error) {
  if (error.code === 'mcp_unreachable') {
    // Use fallback or retry
    console.log('MCP is down, using cache');
  } else if (error.code === 'rate_limited') {
    // Wait and retry
    await sleep(error.retryAfter * 1000);
  } else {
    throw error;
  }
}
```

## Advanced Usage

### Custom Timeout

For long-running tools:

```bash
curl -X POST "https://api.plexmcp.com/mcp" \
  -H "Authorization: ApiKey $PLEXMCP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "mcp_id": "mcp_reports",
    "tool": "generate_report",
    "arguments": {"query": "sales 2024"},
    "timeout": 60000
  }'
```

### Streaming Responses

For tools that return large or progressive output:

```typescript
const stream = await client.mcp.invokeStream({
  mcpId: 'mcp_llm',
  tool: 'generate_text',
  arguments: { prompt: 'Write a story...' },
});

for await (const chunk of stream) {
  process.stdout.write(chunk);
}
```

### Batch Invocations

Call multiple tools efficiently:

```typescript
const results = await Promise.all([
  client.mcp.invoke({
    mcpId: 'mcp_weather',
    tool: 'get_weather',
    arguments: { location: 'SF' },
  }),
  client.mcp.invoke({
    mcpId: 'mcp_weather',
    tool: 'get_forecast',
    arguments: { location: 'SF', days: 5 },
  }),
]);
```

## Best Practices

### 1. Validate Arguments

Check tool schema before invoking:

```typescript
const mcp = await client.mcps.get('mcp_weather');
const tool = mcp.tools.find(t => t.name === 'get_weather');
// Validate arguments against tool.input_schema
```

### 2. Handle Failures Gracefully

```typescript
async function invokeWithFallback(params) {
  try {
    return await client.mcp.invoke(params);
  } catch (error) {
    if (error.code === 'mcp_unreachable') {
      return getCachedResult(params);
    }
    throw error;
  }
}
```

### 3. Set Appropriate Timeouts

- Quick lookups: 5-10 seconds
- Data processing: 30-60 seconds
- Report generation: 60-120 seconds

### 4. Monitor Usage

Check dashboard for:
- Slow tool calls
- High error rates
- Usage patterns

## Testing Tools

### From Dashboard

1. Go to **MCPs** → Select MCP → **Tools** tab
2. Click a tool
3. Enter test arguments
4. Click **Run Test**

### From CLI

```bash
# Install PlexMCP CLI
npm install -g @plexmcp/cli

# Test a tool
plexmcp invoke mcp_weather get_weather --location "San Francisco"
```
