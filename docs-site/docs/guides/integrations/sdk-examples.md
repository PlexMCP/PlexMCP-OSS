---
sidebar_position: 2
---

# SDK Examples

:::note SDKs (Planned)
Official SDKs are under development and will be released incrementally.
The examples below show the planned SDK interface. For now, use the REST API directly with your HTTP client of choice. See the [cURL Examples](#curl) section at the bottom for working examples.
:::

Code examples for integrating PlexMCP into your applications.

## Planned SDKs

- **TypeScript/JavaScript**: `npm install @plexmcp/sdk`
- **Python**: `pip install plexmcp`
- **Go**: `go get github.com/plexmcp/plexmcp-go`

## TypeScript/JavaScript

### Installation

```bash
npm install @plexmcp/sdk
# or
yarn add @plexmcp/sdk
# or
pnpm add @plexmcp/sdk
```

### Basic Usage

```typescript
import { PlexMCP } from '@plexmcp/sdk';

const client = new PlexMCP({
  apiKey: process.env.PLEXMCP_API_KEY,
});

// List all MCPs
const mcps = await client.mcps.list();
console.log(mcps);

// Invoke a tool
const result = await client.mcp.invoke({
  mcpId: 'mcp_123',
  tool: 'get_weather',
  arguments: {
    location: 'San Francisco',
  },
});
console.log(result);
```

### Error Handling

```typescript
import { PlexMCP, PlexMCPError } from '@plexmcp/sdk';

const client = new PlexMCP({
  apiKey: process.env.PLEXMCP_API_KEY,
});

try {
  const result = await client.mcp.invoke({
    mcpId: 'mcp_123',
    tool: 'get_weather',
    arguments: { location: 'SF' },
  });
} catch (error) {
  if (error instanceof PlexMCPError) {
    switch (error.code) {
      case 'rate_limited':
        console.log('Rate limited, waiting...');
        await sleep(error.retryAfter * 1000);
        break;
      case 'mcp_unreachable':
        console.log('MCP is down');
        break;
      default:
        console.error('API error:', error.message);
    }
  } else {
    throw error;
  }
}
```

### Streaming Responses

```typescript
const stream = await client.mcp.invokeStream({
  mcpId: 'mcp_123',
  tool: 'generate_report',
  arguments: { query: 'sales data' },
});

for await (const chunk of stream) {
  process.stdout.write(chunk);
}
```

## Python

### Installation

```bash
pip install plexmcp
```

### Basic Usage

```python
import os
from plexmcp import PlexMCP

client = PlexMCP(api_key=os.environ["PLEXMCP_API_KEY"])

# List all MCPs
mcps = client.mcps.list()
for mcp in mcps:
    print(f"{mcp.name}: {mcp.status}")

# Invoke a tool
result = client.mcp.invoke(
    mcp_id="mcp_123",
    tool="get_weather",
    arguments={"location": "San Francisco"}
)
print(result)
```

### Async Usage

```python
import asyncio
from plexmcp import AsyncPlexMCP

async def main():
    client = AsyncPlexMCP(api_key=os.environ["PLEXMCP_API_KEY"])

    result = await client.mcp.invoke(
        mcp_id="mcp_123",
        tool="get_weather",
        arguments={"location": "San Francisco"}
    )
    print(result)

asyncio.run(main())
```

### Error Handling

```python
from plexmcp import PlexMCP, PlexMCPError, RateLimitError

client = PlexMCP(api_key=os.environ["PLEXMCP_API_KEY"])

try:
    result = client.mcp.invoke(
        mcp_id="mcp_123",
        tool="get_weather",
        arguments={"location": "SF"}
    )
except RateLimitError as e:
    print(f"Rate limited, retry after {e.retry_after} seconds")
    time.sleep(e.retry_after)
except PlexMCPError as e:
    print(f"API error: {e.code} - {e.message}")
```

## Go

### Installation

```bash
go get github.com/PlexMCP/PlexMCP-OSS-go
```

### Basic Usage

```go
package main

import (
    "context"
    "fmt"
    "os"

    "github.com/PlexMCP/PlexMCP-OSS-go"
)

func main() {
    client := plexmcp.NewClient(os.Getenv("PLEXMCP_API_KEY"))

    // List MCPs
    mcps, err := client.MCPs.List(context.Background(), nil)
    if err != nil {
        panic(err)
    }

    for _, mcp := range mcps {
        fmt.Printf("%s: %s\n", mcp.Name, mcp.Status)
    }

    // Invoke a tool
    result, err := client.MCP.Invoke(context.Background(), &plexmcp.InvokeRequest{
        MCPID: "mcp_123",
        Tool:  "get_weather",
        Arguments: map[string]interface{}{
            "location": "San Francisco",
        },
    })
    if err != nil {
        panic(err)
    }

    fmt.Println(result)
}
```

### Error Handling

```go
result, err := client.MCP.Invoke(ctx, req)
if err != nil {
    var apiErr *plexmcp.APIError
    if errors.As(err, &apiErr) {
        switch apiErr.Code {
        case "rate_limited":
            time.Sleep(time.Duration(apiErr.RetryAfter) * time.Second)
            // Retry
        case "mcp_unreachable":
            log.Println("MCP is down")
        default:
            log.Printf("API error: %s", apiErr.Message)
        }
    } else {
        return err
    }
}
```

## cURL

### List MCPs

```bash
curl -X GET "https://api.plexmcp.com/v1/mcps" \
  -H "Authorization: ApiKey $PLEXMCP_API_KEY"
```

### Invoke Tool

```bash
curl -X POST "https://api.plexmcp.com/mcp" \
  -H "Authorization: ApiKey $PLEXMCP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "mcp_id": "mcp_123",
    "tool": "get_weather",
    "arguments": {
      "location": "San Francisco"
    }
  }'
```

### With jq for Pretty Output

```bash
curl -s -X POST "https://api.plexmcp.com/mcp" \
  -H "Authorization: ApiKey $PLEXMCP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "mcp_id": "mcp_123",
    "tool": "get_weather",
    "arguments": {"location": "SF"}
  }' | jq .
```

## Framework Integrations

### Next.js API Route

```typescript
// app/api/weather/route.ts
import { PlexMCP } from '@plexmcp/sdk';
import { NextResponse } from 'next/server';

const client = new PlexMCP({
  apiKey: process.env.PLEXMCP_API_KEY,
});

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url);
  const location = searchParams.get('location');

  const result = await client.mcp.invoke({
    mcpId: 'mcp_weather',
    tool: 'get_weather',
    arguments: { location },
  });

  return NextResponse.json(result);
}
```

### FastAPI

```python
from fastapi import FastAPI
from plexmcp import PlexMCP

app = FastAPI()
client = PlexMCP(api_key=os.environ["PLEXMCP_API_KEY"])

@app.get("/weather/{location}")
async def get_weather(location: str):
    result = await client.mcp.invoke(
        mcp_id="mcp_weather",
        tool="get_weather",
        arguments={"location": location}
    )
    return result
```

### Express.js

```javascript
const express = require('express');
const { PlexMCP } = require('@plexmcp/sdk');

const app = express();
const client = new PlexMCP({
  apiKey: process.env.PLEXMCP_API_KEY,
});

app.get('/weather/:location', async (req, res) => {
  try {
    const result = await client.mcp.invoke({
      mcpId: 'mcp_weather',
      tool: 'get_weather',
      arguments: { location: req.params.location },
    });
    res.json(result);
  } catch (error) {
    res.status(500).json({ error: error.message });
  }
});

app.listen(3000);
```
