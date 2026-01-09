# Self-Hosting PlexMCP

This guide covers everything you need to self-host PlexMCP on your own infrastructure.

## Quick Start

### Option 1: Pre-built Images (Recommended)

The fastest way to get started with pre-built multi-architecture images:

```bash
# Clone the repository
git clone https://github.com/PlexMCP/PlexMCP-OSS.git
cd plexmcp

# Run setup to generate secrets and create .env
./scripts/setup.sh

# Start with pre-built images
docker compose --profile prebuilt up -d

# Verify services are healthy
./scripts/health-check.sh

# Open http://localhost:3000
```

### Option 2: Build from Source

If you want to build the images yourself:

```bash
# Clone the repository
git clone https://github.com/PlexMCP/PlexMCP-OSS.git
cd plexmcp

# Run setup
./scripts/setup.sh

# Build and start from source
docker compose --profile build up -d

# Or use the build script for more control
./scripts/build.sh --tag local
docker compose --profile dev up -d
```

## System Requirements

### Minimum Requirements

| Resource | Requirement |
|----------|-------------|
| CPU | 2 cores |
| RAM | 4 GB |
| Storage | 20 GB |
| Docker | 24.0+ |
| Docker Compose | 2.20+ |

### Recommended for Production

| Resource | Requirement |
|----------|-------------|
| CPU | 4+ cores |
| RAM | 8+ GB |
| Storage | 50+ GB SSD |
| Docker | Latest |

## Platform Compatibility

PlexMCP provides multi-architecture Docker images that work on:

| Platform | Architecture | Status |
|----------|--------------|--------|
| Linux x86_64 | amd64 | ✅ Supported |
| Linux ARM64 | arm64 | ✅ Supported |
| macOS Intel | amd64 | ✅ Supported |
| macOS Apple Silicon | arm64 | ✅ Supported |
| Windows (WSL2) | amd64/arm64 | ✅ Supported |
| Raspberry Pi 4+ | arm64 | ✅ Supported |

## Configuration

### Environment Variables

All configuration is done through environment variables. Run `./scripts/setup.sh` to generate a `.env` file with secure defaults.

#### Required Variables

| Variable | Description |
|----------|-------------|
| `JWT_SECRET` | Secret for signing JWT tokens (32+ hex chars) |
| `API_KEY_HMAC_SECRET` | Secret for API key validation (32+ hex chars) |
| `TOTP_ENCRYPTION_KEY` | Secret for 2FA encryption (64 hex chars) |
| `POSTGRES_PASSWORD` | PostgreSQL database password |

#### Optional Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PUBLIC_URL` | `http://localhost:8080` | Public URL of the API |
| `APP_URL` | `http://localhost:3000` | Public URL of the web app |
| `JWT_EXPIRY_HOURS` | `24` | JWT token expiry time |
| `ENABLE_SIGNUP` | `true` | Allow new user registration |
| `RUST_LOG` | `info,plexmcp=debug` | Logging level |
| `MCP_REQUEST_TIMEOUT_MS` | `30000` | MCP request timeout |
| `MCP_MAX_CONNECTIONS_PER_ORG` | `100` | Max connections per org |

### Port Configuration

| Service | Default Port | Environment Variable |
|---------|--------------|---------------------|
| API | 8080 | `API_PORT` |
| Web | 3000 | `WEB_PORT` |
| PostgreSQL | 5432 | `POSTGRES_PORT` |
| Redis | 6379 | `REDIS_PORT` |

If you have port conflicts, set the environment variables before starting:

```bash
API_PORT=8081 WEB_PORT=3001 docker compose --profile prebuilt up -d
```

## Deployment Options

### Docker Compose Profiles

PlexMCP uses Docker Compose profiles to support different deployment modes:

| Profile | Description | Command |
|---------|-------------|---------|
| `prebuilt` | Use pre-built images from GHCR | `docker compose --profile prebuilt up -d` |
| `build` | Build images from source | `docker compose --profile build up -d` |
| `dev` | Development mode (single arch) | `docker compose --profile dev up -d` |

### Specific Version

To use a specific version instead of `latest`:

```bash
export PLEXMCP_VERSION=v1.0.0
docker compose --profile prebuilt up -d
```

### Production Deployment

For production, we recommend:

1. **Use a reverse proxy** (nginx, Traefik, Caddy) for TLS termination
2. **Set proper URLs** in your `.env`:
   ```bash
   PUBLIC_URL=https://api.yourdomain.com
   APP_URL=https://app.yourdomain.com
   BASE_DOMAIN=yourdomain.com
   ```
3. **Use managed databases** (optional but recommended for reliability)
4. **Set up monitoring** and alerting
5. **Configure backups** for PostgreSQL data

### Example: Traefik Reverse Proxy

```yaml
# Add to docker-compose.yml
services:
  traefik:
    image: traefik:v3.0
    command:
      - --providers.docker
      - --entrypoints.web.address=:80
      - --entrypoints.websecure.address=:443
      - --certificatesresolvers.letsencrypt.acme.email=you@example.com
      - --certificatesresolvers.letsencrypt.acme.storage=/letsencrypt/acme.json
      - --certificatesresolvers.letsencrypt.acme.httpchallenge.entrypoint=web
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - letsencrypt:/letsencrypt

  api:
    labels:
      - traefik.enable=true
      - traefik.http.routers.api.rule=Host(`api.yourdomain.com`)
      - traefik.http.routers.api.tls.certresolver=letsencrypt

  web:
    labels:
      - traefik.enable=true
      - traefik.http.routers.web.rule=Host(`app.yourdomain.com`)
      - traefik.http.routers.web.tls.certresolver=letsencrypt
```

## Building Multi-Architecture Images

To build images for multiple architectures:

```bash
# Build for both amd64 and arm64
./scripts/build.sh --platform "linux/amd64,linux/arm64" --tag v1.0.0

# Push to your own registry
REGISTRY=your-registry.com/plexmcp ./scripts/build.sh --push --tag v1.0.0
```

## Troubleshooting

### Services Won't Start

1. Check Docker is running:
   ```bash
   docker info
   ```

2. Check for port conflicts:
   ```bash
   lsof -i :8080
   lsof -i :3000
   lsof -i :5432
   lsof -i :6379
   ```

3. Check logs:
   ```bash
   docker compose --profile prebuilt logs api
   docker compose --profile prebuilt logs web
   ```

### Database Connection Issues

1. Verify PostgreSQL is healthy:
   ```bash
   docker compose --profile prebuilt ps postgres
   ```

2. Check the connection string in `.env`

3. Try resetting the database:
   ```bash
   docker compose --profile prebuilt down -v
   docker compose --profile prebuilt up -d
   ```

### Build Fails on Apple Silicon

If building from source fails, ensure:

1. Docker Desktop has Rosetta emulation enabled (for x86 images)
2. Use the native arm64 build:
   ```bash
   docker compose --profile dev up -d
   ```

### Health Check Failures

Run the health check script for detailed diagnostics:

```bash
./scripts/health-check.sh --verbose
```

### Memory Issues

If containers are being killed (OOMKilled):

1. Increase Docker memory limits (Docker Desktop settings)
2. Reduce concurrent connections in `.env`:
   ```bash
   MCP_MAX_CONNECTIONS_PER_ORG=50
   ```

## Data Management

### Backup

Backup your PostgreSQL data:

```bash
docker compose --profile prebuilt exec postgres pg_dump -U plexmcp plexmcp > backup.sql
```

### Restore

Restore from backup:

```bash
docker compose --profile prebuilt exec -T postgres psql -U plexmcp plexmcp < backup.sql
```

### Upgrade

To upgrade to a new version:

```bash
# Pull new images
export PLEXMCP_VERSION=v1.1.0
docker compose --profile prebuilt pull

# Restart with new images
docker compose --profile prebuilt up -d

# Check health
./scripts/health-check.sh
```

## Security Considerations

1. **Rotate secrets regularly** - Regenerate JWT_SECRET and other keys periodically
2. **Use TLS** - Always use HTTPS in production
3. **Firewall** - Only expose necessary ports (80/443 through reverse proxy)
4. **Updates** - Keep Docker and PlexMCP updated
5. **Backups** - Regularly backup your database
6. **Monitoring** - Set up alerts for failed health checks

## Support

- **Documentation**: https://docs.plexmcp.com
- **GitHub Issues**: https://github.com/PlexMCP/PlexMCP-OSS/issues
- **Discussions**: https://github.com/PlexMCP/PlexMCP-OSS/discussions

## License

PlexMCP is licensed under the [Functional Source License (FSL-1.1-Apache-2.0)](./LICENSE).

- **Self-hosting**: Always permitted for any organization
- **Commercial use** (>$1M revenue): Requires commercial license
- **Converts to Apache 2.0**: January 6, 2031

See [COMMERCIAL_LICENSE.md](./COMMERCIAL_LICENSE.md) for details.
