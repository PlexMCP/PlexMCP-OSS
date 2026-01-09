---
sidebar_position: 6
---

# Upgrading PlexMCP

This guide covers how to upgrade your self-hosted PlexMCP installation.

## Before Upgrading

### 1. Check Release Notes

Always read the release notes before upgrading:

```bash
# View latest releases
curl -s https://api.github.com/repos/PlexMCP/plexmcp/releases/latest | jq '.tag_name, .body'
```

Or visit: https://github.com/PlexMCP/PlexMCP-OSS/releases

### 2. Backup Your Data

**Always backup before upgrading:**

```bash
# Quick backup
./scripts/backup.sh

# Or manually
docker compose exec postgres pg_dump -U plexmcp plexmcp > backup-$(date +%Y%m%d).sql
```

See [Backup Guide](./backup.md) for detailed instructions.

### 3. Check Compatibility

- Review breaking changes in release notes
- Check for new required environment variables
- Verify your PostgreSQL/Redis versions are still supported

## Docker Compose Upgrade

### Standard Upgrade

```bash
# Navigate to PlexMCP directory
cd /path/to/plexmcp

# Pull latest changes
git fetch origin
git checkout main
git pull origin main

# Stop services
docker compose down

# Rebuild images
docker compose build --no-cache

# Start services
docker compose up -d

# Run database migrations
docker compose exec api sqlx migrate run

# Verify health
docker compose ps
curl http://localhost:8080/health
```

### Upgrade to Specific Version

```bash
# List available versions
git tag -l

# Checkout specific version
git checkout v1.2.0

# Rebuild and restart
docker compose down
docker compose build --no-cache
docker compose up -d
docker compose exec api sqlx migrate run
```

### Zero-Downtime Upgrade

For production environments with minimal downtime:

```bash
# Pull new images in background
docker compose pull

# Build new images
docker compose build

# Recreate containers one by one
docker compose up -d --no-deps --build api
docker compose up -d --no-deps --build web

# Run migrations
docker compose exec api sqlx migrate run
```

## Manual Deployment Upgrade

### 1. Stop Services

```bash
sudo systemctl stop plexmcp-api plexmcp-web
```

### 2. Backup Current Version

```bash
# Backup current binaries
cp /opt/plexmcp/target/release/plexmcp-api /opt/plexmcp/plexmcp-api.bak
cp -r /opt/plexmcp/web/.next /opt/plexmcp/web/.next.bak
```

### 3. Pull Latest Code

```bash
cd /opt/plexmcp
git fetch origin
git pull origin main
```

### 4. Rebuild Backend

```bash
# Build new release
cargo build --release

# Verify binary
./target/release/plexmcp-api --version
```

### 5. Rebuild Frontend

```bash
cd web
npm install
npm run build
cd ..
```

### 6. Run Migrations

```bash
sqlx migrate run
```

### 7. Restart Services

```bash
sudo systemctl start plexmcp-api plexmcp-web
sudo systemctl status plexmcp-api plexmcp-web
```

## Database Migrations

### Automatic Migrations

Migrations run automatically when using:

```bash
# Docker
docker compose exec api sqlx migrate run

# Manual
sqlx migrate run
```

### Checking Migration Status

```bash
# List applied migrations
sqlx migrate info

# Or via SQL
psql -U plexmcp -d plexmcp -c "SELECT * FROM _sqlx_migrations ORDER BY installed_on"
```

### Rolling Back Migrations

**Use with caution - may cause data loss:**

```bash
# Revert last migration
sqlx migrate revert

# Revert to specific version (if supported)
sqlx migrate revert --target-version 20240101000000
```

## Handling Breaking Changes

### New Required Environment Variables

If a release introduces new required variables:

1. Check `.env.example` for new variables
2. Add them to your `.env` file
3. Regenerate secrets if needed:

```bash
# Generate new secret
openssl rand -hex 32
```

### Database Schema Changes

Major schema changes may require:

```bash
# 1. Backup data
pg_dump -U plexmcp plexmcp > backup.sql

# 2. Run migrations
sqlx migrate run

# 3. Verify data integrity
psql -U plexmcp -d plexmcp -c "SELECT COUNT(*) FROM users"
```

### Configuration Changes

If config format changes:

```bash
# Compare with example
diff .env .env.example

# Update as needed
nano .env
```

## Rollback Procedures

### Docker Rollback

```bash
# Stop current version
docker compose down

# Checkout previous version
git checkout v1.1.0

# Rebuild and start
docker compose build --no-cache
docker compose up -d

# Restore database if needed
cat backup.sql | docker compose exec -T postgres psql -U plexmcp plexmcp
```

### Manual Rollback

```bash
# Stop services
sudo systemctl stop plexmcp-api plexmcp-web

# Restore binaries
cp /opt/plexmcp/plexmcp-api.bak /opt/plexmcp/target/release/plexmcp-api
cp -r /opt/plexmcp/web/.next.bak /opt/plexmcp/web/.next

# Restore database if needed
psql -U plexmcp plexmcp < backup.sql

# Restart services
sudo systemctl start plexmcp-api plexmcp-web
```

## Version Compatibility Matrix

| PlexMCP Version | PostgreSQL | Redis | Node.js | Rust |
|-----------------|------------|-------|---------|------|
| 1.0.x | 15-16 | 7+ | 20+ | 1.75+ |
| 1.1.x | 15-17 | 7+ | 20+ | 1.75+ |

## Upgrade Checklist

### Pre-Upgrade

- [ ] Read release notes
- [ ] Backup database
- [ ] Backup .env file
- [ ] Note current version
- [ ] Schedule maintenance window (if production)
- [ ] Notify users (if production)

### During Upgrade

- [ ] Stop services
- [ ] Pull latest code
- [ ] Update .env if needed
- [ ] Rebuild binaries/images
- [ ] Run migrations
- [ ] Start services

### Post-Upgrade

- [ ] Verify health endpoints
- [ ] Test authentication
- [ ] Test MCP connections
- [ ] Check logs for errors
- [ ] Verify data integrity
- [ ] Update monitoring/alerting

## Troubleshooting Upgrades

### Migration Fails

```bash
# Check migration error
docker compose logs api | grep -i migration

# Common fixes:
# - Database connection issue: verify DATABASE_URL
# - Permission issue: check database user permissions
# - Conflict: manually resolve in _sqlx_migrations table
```

### Service Won't Start After Upgrade

```bash
# Check logs
docker compose logs api

# Common issues:
# - Missing env var: check .env.example for new required vars
# - Port conflict: ensure old containers are stopped
# - Database schema mismatch: ensure migrations ran
```

### Performance Issues After Upgrade

```bash
# Check resource usage
docker stats

# Rebuild indexes if needed
docker compose exec postgres psql -U plexmcp -c "REINDEX DATABASE plexmcp"

# Analyze tables
docker compose exec postgres psql -U plexmcp -c "ANALYZE"
```

## Automated Upgrades

### Watchtower (Docker)

For automatic container updates (use with caution):

```yaml
# docker-compose.yml
services:
  watchtower:
    image: containrrr/watchtower
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
    command: --interval 86400 --cleanup
```

**Warning:** Automated upgrades can be risky. Recommended only for non-critical deployments.

### GitHub Actions

Set up notifications for new releases:

```yaml
# .github/workflows/notify-upgrade.yml
name: Notify New Release
on:
  release:
    types: [published]
jobs:
  notify:
    runs-on: ubuntu-latest
    steps:
      - name: Send notification
        run: |
          curl -X POST ${{ secrets.WEBHOOK_URL }} \
            -H "Content-Type: application/json" \
            -d '{"text": "New PlexMCP release: ${{ github.event.release.tag_name }}"}'
```

## Next Steps

- [Backup & Restore →](./backup.md)
- [Configuration Reference →](./configuration.md)
- [Troubleshooting →](./docker.md#troubleshooting)
