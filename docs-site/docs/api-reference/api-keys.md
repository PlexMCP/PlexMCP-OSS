---
sidebar_position: 5
---

# API Keys Endpoints

Endpoints for managing API keys programmatically.

## List API Keys

Get all API keys for your organization.

```http
GET /v1/api-keys
```

### Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `page` | integer | Page number (default: 1) |
| `per_page` | integer | Items per page (default: 20, max: 100) |
| `status` | string | Filter: `active`, `revoked`, `expired` |

### Response

```json
{
  "success": true,
  "data": [
    {
      "id": "key_123",
      "name": "Production Server",
      "description": "Main production API key",
      "prefix": "pmcp_abc",
      "status": "active",
      "created_at": "2024-01-15T10:30:00Z",
      "last_used": "2024-01-20T15:45:00Z",
      "expires_at": null,
      "permissions": {
        "mcps": "*"
      },
      "usage": {
        "total_requests": 5000,
        "requests_today": 250
      }
    }
  ],
  "pagination": {
    "page": 1,
    "per_page": 20,
    "total": 2,
    "total_pages": 1
  }
}
```

## Get API Key Details

Get details about a specific API key.

```http
GET /v1/api-keys/{key_id}
```

### Response

```json
{
  "success": true,
  "data": {
    "id": "key_123",
    "name": "Production Server",
    "description": "Main production API key",
    "prefix": "pmcp_abc",
    "status": "active",
    "created_at": "2024-01-15T10:30:00Z",
    "created_by": {
      "id": "user_123",
      "email": "alice@example.com"
    },
    "last_used": "2024-01-20T15:45:00Z",
    "expires_at": null,
    "permissions": {
      "mcps": "*"
    },
    "usage": {
      "total_requests": 5000,
      "requests_today": 250,
      "requests_this_week": 1200,
      "requests_this_month": 5000
    },
    "recent_activity": [
      {
        "timestamp": "2024-01-20T15:45:00Z",
        "mcp_id": "mcp_123",
        "tool": "get_weather",
        "status": "success"
      }
    ]
  }
}
```

## Create API Key

Generate a new API key.

```http
POST /v1/api-keys
```

### Request Body

```json
{
  "name": "Development Key",
  "description": "For local development",
  "expires_at": "2024-06-15T00:00:00Z",
  "permissions": {
    "mcps": ["mcp_123", "mcp_456"]
  }
}
```

### Permissions Options

**All MCPs:**
```json
{
  "permissions": {
    "mcps": "*"
  }
}
```

**Specific MCPs:**
```json
{
  "permissions": {
    "mcps": ["mcp_123", "mcp_456"]
  }
}
```

### Response

```json
{
  "success": true,
  "data": {
    "id": "key_789",
    "name": "Development Key",
    "key": "pmcp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    "prefix": "pmcp_xxx",
    "status": "active",
    "created_at": "2024-01-20T16:00:00Z",
    "expires_at": "2024-06-15T00:00:00Z",
    "permissions": {
      "mcps": ["mcp_123", "mcp_456"]
    }
  }
}
```

:::warning
The full API key is only returned once during creation. Store it securely before leaving this page.
:::

## Update API Key

Update API key metadata.

```http
PATCH /v1/api-keys/{key_id}
```

### Request Body

```json
{
  "name": "Updated Key Name",
  "description": "Updated description"
}
```

Note: Permissions and expiration cannot be modified after creation. Create a new key instead.

### Response

```json
{
  "success": true,
  "data": {
    "id": "key_123",
    "name": "Updated Key Name",
    "description": "Updated description",
    "updated_at": "2024-01-20T16:00:00Z"
  }
}
```

## Revoke API Key

Immediately disable an API key.

```http
DELETE /v1/api-keys/{key_id}
```

### Response

```json
{
  "success": true,
  "data": {
    "id": "key_123",
    "status": "revoked",
    "revoked_at": "2024-01-20T16:00:00Z"
  }
}
```

## Get Key Usage Statistics

Get detailed usage for a specific key.

```http
GET /v1/api-keys/{key_id}/usage
```

### Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `period` | string | `day`, `week`, `month` |
| `start_date` | string | Start date (ISO 8601) |
| `end_date` | string | End date (ISO 8601) |

### Response

```json
{
  "success": true,
  "data": {
    "key_id": "key_123",
    "period": "week",
    "total_requests": 1200,
    "by_mcp": [
      {
        "mcp_id": "mcp_123",
        "name": "Weather API",
        "requests": 800
      },
      {
        "mcp_id": "mcp_456",
        "name": "Calculator",
        "requests": 400
      }
    ],
    "by_day": [
      { "date": "2024-01-14", "requests": 150 },
      { "date": "2024-01-15", "requests": 180 },
      { "date": "2024-01-16", "requests": 200 }
    ],
    "by_status": {
      "success": 1150,
      "error": 50
    }
  }
}
```

## Errors

### 400 Bad Request

```json
{
  "success": false,
  "error": {
    "code": "invalid_request",
    "message": "Invalid permissions format"
  }
}
```

### 403 Forbidden

```json
{
  "success": false,
  "error": {
    "code": "forbidden",
    "message": "Cannot revoke your only active key"
  }
}
```

### 404 Not Found

```json
{
  "success": false,
  "error": {
    "code": "not_found",
    "message": "API key not found"
  }
}
```

### 422 Validation Error

```json
{
  "success": false,
  "error": {
    "code": "validation_error",
    "message": "Validation failed",
    "details": {
      "expires_at": "Must be a future date"
    }
  }
}
```
