---
sidebar_position: 5
---

# Manual Deployment

This guide covers deploying PlexMCP without Docker, useful for custom infrastructure or existing server setups.

## Prerequisites

### System Requirements

- Linux server (Ubuntu 22.04 LTS recommended)
- PostgreSQL 15+ (16 recommended)
- Redis 7+
- Rust 1.75+ (for building from source)
- Node.js 20+ (for frontend)

### Build Tools

```bash
# Ubuntu/Debian
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Install Node.js 20
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt install -y nodejs
```

## Database Setup

### Install PostgreSQL

```bash
# Install PostgreSQL 16
sudo apt install -y postgresql-16 postgresql-contrib-16

# Start PostgreSQL
sudo systemctl enable postgresql
sudo systemctl start postgresql
```

### Create Database

```bash
# Switch to postgres user
sudo -u postgres psql

# Create user and database
CREATE USER plexmcp WITH PASSWORD 'your_secure_password';
CREATE DATABASE plexmcp OWNER plexmcp;

# Enable required extensions
\c plexmcp
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

\q
```

### Configure PostgreSQL

Edit `/etc/postgresql/16/main/pg_hba.conf` to allow local connections:

```
# IPv4 local connections:
host    plexmcp         plexmcp         127.0.0.1/32            scram-sha-256
```

Restart PostgreSQL:

```bash
sudo systemctl restart postgresql
```

## Redis Setup

### Install Redis

```bash
# Install Redis
sudo apt install -y redis-server

# Enable and start
sudo systemctl enable redis-server
sudo systemctl start redis-server
```

### Configure Redis (Optional)

For production, edit `/etc/redis/redis.conf`:

```conf
# Enable persistence
appendonly yes
appendfsync everysec

# Set memory limit
maxmemory 512mb
maxmemory-policy allkeys-lru

# Bind to localhost only
bind 127.0.0.1
```

## Build PlexMCP

### Clone Repository

```bash
git clone https://github.com/PlexMCP/PlexMCP-OSS.git
cd plexmcp
```

### Build Backend (API)

```bash
# Build release binary
cargo build --release

# Binary location
ls -la target/release/plexmcp-api
```

### Build Frontend

```bash
cd web

# Install dependencies
npm install

# Build for production
npm run build

cd ..
```

## Configuration

### Generate Secrets

```bash
# Generate all secrets
JWT_SECRET=$(openssl rand -hex 32)
API_KEY_HMAC_SECRET=$(openssl rand -hex 32)
TOTP_ENCRYPTION_KEY=$(openssl rand -hex 32)

echo "JWT_SECRET=$JWT_SECRET"
echo "API_KEY_HMAC_SECRET=$API_KEY_HMAC_SECRET"
echo "TOTP_ENCRYPTION_KEY=$TOTP_ENCRYPTION_KEY"
```

### Create Environment File

```bash
cat > .env << EOF
# Database
DATABASE_URL=postgresql://plexmcp:your_secure_password@localhost:5432/plexmcp

# Redis
REDIS_URL=redis://localhost:6379

# Server
BIND_ADDRESS=0.0.0.0:8080
PUBLIC_URL=https://api.yourdomain.com
BASE_DOMAIN=yourdomain.com

# Authentication
JWT_SECRET=$JWT_SECRET
JWT_EXPIRY_HOURS=24
API_KEY_HMAC_SECRET=$API_KEY_HMAC_SECRET
TOTP_ENCRYPTION_KEY=$TOTP_ENCRYPTION_KEY

# Self-hosted mode
PLEXMCP_SELF_HOSTED=true
ENABLE_BILLING=false
ENABLE_SIGNUP=true

# Logging
RUST_LOG=info,plexmcp=debug
EOF
```

### Run Migrations

```bash
# Install sqlx-cli if not already installed
cargo install sqlx-cli

# Run migrations
sqlx migrate run
```

## Running Services

### Systemd Service for API

Create `/etc/systemd/system/plexmcp-api.service`:

```ini
[Unit]
Description=PlexMCP API Server
After=network.target postgresql.service redis-server.service

[Service]
Type=simple
User=plexmcp
Group=plexmcp
WorkingDirectory=/opt/plexmcp
ExecStart=/opt/plexmcp/target/release/plexmcp-api
Restart=always
RestartSec=5
Environment=RUST_LOG=info,plexmcp=debug
EnvironmentFile=/opt/plexmcp/.env

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/plexmcp

[Install]
WantedBy=multi-user.target
```

### Systemd Service for Frontend

Create `/etc/systemd/system/plexmcp-web.service`:

```ini
[Unit]
Description=PlexMCP Web Frontend
After=network.target plexmcp-api.service

[Service]
Type=simple
User=plexmcp
Group=plexmcp
WorkingDirectory=/opt/plexmcp/web
ExecStart=/usr/bin/node .next/standalone/server.js
Restart=always
RestartSec=5
Environment=NODE_ENV=production
Environment=PORT=3000
Environment=HOSTNAME=0.0.0.0
EnvironmentFile=/opt/plexmcp/.env

# Security hardening
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

### Enable and Start Services

```bash
# Create plexmcp user
sudo useradd -r -s /bin/false plexmcp

# Set ownership
sudo chown -R plexmcp:plexmcp /opt/plexmcp

# Reload systemd
sudo systemctl daemon-reload

# Enable services
sudo systemctl enable plexmcp-api plexmcp-web

# Start services
sudo systemctl start plexmcp-api plexmcp-web

# Check status
sudo systemctl status plexmcp-api plexmcp-web
```

## Reverse Proxy Setup

### Nginx

Install and configure nginx:

```bash
sudo apt install -y nginx
```

Create `/etc/nginx/sites-available/plexmcp`:

```nginx
# Rate limiting zone
limit_req_zone $binary_remote_addr zone=api:10m rate=10r/s;

server {
    listen 80;
    server_name yourdomain.com api.yourdomain.com;
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    server_name yourdomain.com;

    ssl_certificate /etc/letsencrypt/live/yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/yourdomain.com/privkey.pem;

    # Frontend
    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_cache_bypass $http_upgrade;
    }
}

server {
    listen 443 ssl http2;
    server_name api.yourdomain.com;

    ssl_certificate /etc/letsencrypt/live/yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/yourdomain.com/privkey.pem;

    # API with rate limiting
    location / {
        limit_req zone=api burst=20 nodelay;

        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # SSE support
        proxy_buffering off;
        proxy_read_timeout 86400s;
    }
}
```

Enable the site:

```bash
sudo ln -s /etc/nginx/sites-available/plexmcp /etc/nginx/sites-enabled/
sudo nginx -t
sudo systemctl reload nginx
```

### SSL with Let's Encrypt

```bash
# Install certbot
sudo apt install -y certbot python3-certbot-nginx

# Get certificate
sudo certbot --nginx -d yourdomain.com -d api.yourdomain.com

# Auto-renewal is enabled by default
sudo systemctl status certbot.timer
```

## Firewall Configuration

```bash
# Allow SSH, HTTP, HTTPS
sudo ufw allow ssh
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp

# Enable firewall
sudo ufw enable

# Check status
sudo ufw status
```

## Monitoring

### Log Viewing

```bash
# API logs
sudo journalctl -u plexmcp-api -f

# Web logs
sudo journalctl -u plexmcp-web -f

# Combined
sudo journalctl -u plexmcp-api -u plexmcp-web -f
```

### Health Checks

```bash
# API health
curl http://localhost:8080/health

# Web health
curl http://localhost:3000/api/health
```

### Resource Monitoring

```bash
# Process status
ps aux | grep plexmcp

# Memory usage
free -h

# Disk usage
df -h
```

## Updating

See [Upgrading Guide](./upgrading.md) for update procedures.

## Troubleshooting

### API won't start

```bash
# Check logs
sudo journalctl -u plexmcp-api -n 50

# Common issues:
# - Database connection: verify DATABASE_URL
# - Port in use: check with `lsof -i :8080`
# - Permission denied: check file ownership
```

### Database connection errors

```bash
# Test PostgreSQL connection
psql -h localhost -U plexmcp -d plexmcp -c "SELECT 1"

# Check PostgreSQL is running
sudo systemctl status postgresql
```

### Redis connection errors

```bash
# Test Redis connection
redis-cli ping

# Check Redis is running
sudo systemctl status redis-server
```

### Frontend build errors

```bash
# Clear cache and rebuild
cd web
rm -rf .next node_modules
npm install
npm run build
```

## Next Steps

- [Configuration Reference →](./configuration.md)
- [Upgrading →](./upgrading.md)
- [Backup & Restore →](./backup.md)
