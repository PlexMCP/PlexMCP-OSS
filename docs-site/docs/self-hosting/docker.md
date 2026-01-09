---
sidebar_position: 3
---

# Docker Deployment

This guide walks you through deploying PlexMCP using Docker Compose.

## Prerequisites

- Docker Engine 24.0+
- Docker Compose 2.20+
- 4GB+ RAM available
- 20GB+ disk space

## Quick Start

### 1. Clone the Repository

```bash
git clone https://github.com/PlexMCP/PlexMCP-OSS.git
cd plexmcp
```

### 2. Run Setup Script

```bash
./scripts/setup.sh
```

This will:
- Generate secure secrets (JWT, HMAC, TOTP keys)
- Create your `.env` file
- Validate the configuration

### 3. Start Services

```bash
docker compose up -d
```

### 4. Verify Deployment

```bash
# Check all services are running
docker compose ps

# View logs
docker compose logs -f
```

### 5. Access PlexMCP

Open http://localhost:3000 in your browser.

## Configuration

### Environment Variables

Key environment variables in `.env`:

```bash
# Database
DATABASE_URL=postgresql://plexmcp:password@postgres:5432/plexmcp
POSTGRES_PASSWORD=your_secure_password

# Authentication (generate with: openssl rand -hex 32)
JWT_SECRET=your_jwt_secret
API_KEY_HMAC_SECRET=your_hmac_secret
TOTP_ENCRYPTION_KEY=your_totp_key

# Self-hosted mode
PLEXMCP_SELF_HOSTED=true
ENABLE_BILLING=false
ENABLE_SIGNUP=true
```

See [Configuration Reference](./configuration.md) for all options.

### Custom Ports

To change the default ports:

```bash
# .env
API_PORT=9000        # Default: 8080
WEB_PORT=4000        # Default: 3000
POSTGRES_PORT=5433   # Default: 5432
REDIS_PORT=6380      # Default: 6379
```

### External Database

To use an external PostgreSQL database:

```bash
# .env
DATABASE_URL=postgresql://user:password@your-host:5432/plexmcp
```

Then remove the `postgres` service from `docker-compose.yml` or use:

```bash
docker compose up -d api web redis
```

### External Redis

Similarly for external Redis:

```bash
# .env
REDIS_URL=redis://your-redis-host:6379
```

## Services

The Docker Compose file includes:

| Service | Description | Port |
|---------|-------------|------|
| `postgres` | PostgreSQL database | 5432 |
| `redis` | Redis cache | 6379 |
| `api` | PlexMCP API server | 8080 |
| `web` | Next.js frontend | 3000 |

## Production Deployment

### Using a Reverse Proxy

For production, use a reverse proxy with SSL:

**nginx example:**

```nginx
server {
    listen 443 ssl http2;
    server_name plexmcp.yourdomain.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    # Frontend
    location / {
        proxy_pass http://localhost:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }

    # API
    location /api {
        proxy_pass http://localhost:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

**Caddy example (automatic SSL):**

```
plexmcp.yourdomain.com {
    reverse_proxy /api/* localhost:8080
    reverse_proxy localhost:3000
}
```

### Health Checks

Both services expose health endpoints:

```bash
# API health
curl http://localhost:8080/health

# Web health
curl http://localhost:3000/api/health
```

### Logging

View logs for specific services:

```bash
# All services
docker compose logs -f

# Specific service
docker compose logs -f api

# Last 100 lines
docker compose logs --tail 100 api
```

### Updating

To update to a new version:

```bash
# Pull latest changes
git pull

# Rebuild and restart
docker compose down
docker compose build --no-cache
docker compose up -d

# Run migrations (if any)
docker compose exec api sqlx migrate run
```

## Troubleshooting

### Services won't start

Check logs:
```bash
docker compose logs api
```

Common issues:
- Database not ready: Wait for PostgreSQL to initialize
- Port conflicts: Check if ports 3000, 8080, 5432, 6379 are available
- Missing secrets: Ensure `.env` file has all required secrets

### Database connection errors

```bash
# Check PostgreSQL is running
docker compose ps postgres

# Test connection
docker compose exec postgres psql -U plexmcp -c "SELECT 1"
```

### API returns 500 errors

```bash
# Check API logs
docker compose logs api

# Verify environment variables
docker compose exec api env | grep -E "(DATABASE|JWT|REDIS)"
```

### Reset everything

```bash
# Stop and remove all containers and volumes
docker compose down -v

# Remove images
docker compose down --rmi all

# Start fresh
./scripts/setup.sh
docker compose up -d
```

## Backups

### Database Backup

```bash
# Create backup
docker compose exec postgres pg_dump -U plexmcp plexmcp > backup.sql

# Restore backup
cat backup.sql | docker compose exec -T postgres psql -U plexmcp plexmcp
```

### Full Backup

```bash
# Backup everything
tar -czvf plexmcp-backup.tar.gz \
  .env \
  docker-compose.yml \
  backup.sql
```

See [Backup Guide](./backup.md) for more details.

## Next Steps

- [Configuration Reference →](./configuration.md)
- [Upgrading →](./upgrading.md)
- [Backup & Restore →](./backup.md)
