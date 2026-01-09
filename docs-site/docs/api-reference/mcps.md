---
sidebar_position: 4
---

# MCPs API

Endpoints for managing and interacting with MCPs.

## List MCPs

Get all MCPs in your organization.

```http
GET /v1/mcps
```

### Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `page` | integer | Page number (default: 1) |
| `per_page` | integer | Items per page (default: 20, max: 100) |
| `status` | string | Filter: `active`, `inactive`, `all` |
| `health` | string | Filter: `healthy`, `unhealthy` |

### Response

```json
{
  "success": true,
  "data": [
    {
      "id": "mcp_123",
      "name": "Weather API",
      "endpoint_url": "https://weather-mcp.example.com",
      "description": "Weather data and forecasts",
      "status": "active",
      "health": "healthy",
      "created_at": "2024-01-15T10:30:00Z",
      "last_active": "2024-01-20T15:45:00Z",
      "request_count": 8000
    }
  ],
  "pagination": {
    "page": 1,
    "per_page": 20,
    "total": 3,
    "total_pages": 1
  }
}
```

## Get MCP Details

Get detailed information about a specific MCP.

```http
GET /v1/mcps/{mcp_id}
```

### Response

```json
{
  "success": true,
  "data": {
    "id": "mcp_123",
    "name": "Weather API",
    "endpoint_url": "https://weather-mcp.example.com",
    "description": "Weather data and forecasts",
    "status": "active",
    "health": "healthy",
    "health_checked_at": "2024-01-20T15:40:00Z",
    "created_at": "2024-01-15T10:30:00Z",
    "updated_at": "2024-01-18T12:00:00Z",
    "last_active": "2024-01-20T15:45:00Z",
    "stats": {
      "total_requests": 8000,
      "requests_today": 450,
      "error_rate": 0.02,
      "avg_latency_ms": 125
    },
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
            }
          },
          "required": ["location"]
        }
      },
      {
        "name": "get_forecast",
        "description": "Get weather forecast",
        "input_schema": {
          "type": "object",
          "properties": {
            "location": { "type": "string" },
            "days": { "type": "integer", "default": 5 }
          },
          "required": ["location"]
        }
      }
    ]
  }
}
```

## Invoke MCP Tool

Call a tool on an MCP. This is the primary endpoint for interacting with your MCPs.

```http
POST /mcp
Content-Type: application/json
```

### Request Body

```json
{
  "mcp_id": "mcp_123",
  "tool": "get_weather",
  "arguments": {
    "location": "San Francisco, CA"
  }
}
```

### Response

```json
{
  "success": true,
  "data": {
    "result": {
      "location": "San Francisco, CA",
      "temperature": 65,
      "unit": "fahrenheit",
      "conditions": "Partly cloudy",
      "humidity": 72
    },
    "metadata": {
      "mcp_id": "mcp_123",
      "tool": "get_weather",
      "latency_ms": 125,
      "timestamp": "2024-01-20T15:45:00Z"
    }
  }
}
```

## Create MCP

Register a new MCP (Admin/Owner only).

```http
POST /v1/mcps
```

### Request Body

```json
{
  "name": "Weather API",
  "endpoint_url": "https://weather-mcp.example.com",
  "description": "Weather data and forecasts",
  "auth_type": "bearer",
  "auth_token": "secret_token"
}
```

### Response

```json
{
  "success": true,
  "data": {
    "id": "mcp_789",
    "name": "Weather API",
    "endpoint_url": "https://weather-mcp.example.com",
    "description": "Weather data and forecasts",
    "status": "active",
    "health": "unknown",
    "created_at": "2024-01-20T16:00:00Z"
  }
}
```

## Update MCP

Update MCP configuration (Admin/Owner only).

```http
PATCH /v1/mcps/{mcp_id}
```

### Request Body

```json
{
  "name": "Updated Weather API",
  "description": "Updated description",
  "status": "active"
}
```

### Response

```json
{
  "success": true,
  "data": {
    "id": "mcp_123",
    "name": "Updated Weather API",
    "description": "Updated description",
    "status": "active",
    "updated_at": "2024-01-20T16:00:00Z"
  }
}
```

## Delete MCP

Remove an MCP (Admin/Owner only).

```http
DELETE /v1/mcps/{mcp_id}
```

### Response

```json
{
  "success": true,
  "data": {
    "deleted": true,
    "mcp_id": "mcp_123"
  }
}
```

## Test MCP Connection

Test connectivity to an MCP.

```http
POST /v1/mcps/{mcp_id}/test
```

### Response

```json
{
  "success": true,
  "data": {
    "reachable": true,
    "latency_ms": 95,
    "protocol_version": "2024-11-05",
    "tools_count": 5,
    "resources_count": 2
  }
}
```

## Errors

### 404 Not Found

```json
{
  "success": false,
  "error": {
    "code": "not_found",
    "message": "MCP not found"
  }
}
```

### 502 Bad Gateway

```json
{
  "success": false,
  "error": {
    "code": "mcp_unreachable",
    "message": "Could not connect to MCP server"
  }
}
```

### 504 Gateway Timeout

```json
{
  "success": false,
  "error": {
    "code": "mcp_timeout",
    "message": "MCP server did not respond in time"
  }
}
```
