# Deployment Guide

Production deployment guide for the enterprise facilitator with Docker, Kubernetes, and reverse proxy configurations.

## Production Checklist

Before deploying to production, complete these items:

### Security
- Enable API key authentication (`API_KEYS`)
- Set admin key (`ADMIN_API_KEY`)
- Restrict CORS origins in `config.toml`
- Enable rate limiting
- Configure IP filtering (if needed)
- Use HTTPS (reverse proxy or load balancer)
- Store private keys in secret manager (AWS/GCP/Vault)

### Configuration
- Set appropriate rate limits for expected traffic
- Configure upstream chain/scheme config (`config.json`) with production RPC endpoints
- Configure enterprise security settings (`config.toml`)
- Enable batch settlement for high throughput (if needed)
- Configure hooks (if needed)

### Monitoring
- Enable OpenTelemetry export
- Set up log aggregation
- Configure alerting for errors and low balances
- Set up uptime monitoring
- Monitor wallet gas balances

### Infrastructure
- Use dedicated RPC providers (Alchemy, Infura, etc.)
- Deploy behind reverse proxy (Nginx, Caddy, Cloudflare)
- Configure health checks
- Set up auto-restart on failure
- Plan for zero-downtime deployments

## Docker Deployment

### Build Image

```bash
# Build from workspace root
docker build -f facilitator-enterprise/Dockerfile -t facilitator-enterprise:latest .
```

### Run Container

```bash
docker run -d \
  --name facilitator \
  -p 8080:8080 \
  --env-file facilitator-enterprise/.env \
  -v $(pwd)/config.json:/app/config.json:ro \
  -v $(pwd)/facilitator-enterprise/config.toml:/app/config.toml:ro \
  -v $(pwd)/facilitator-enterprise/hooks.toml:/app/hooks.toml:ro \
  -v $(pwd)/facilitator-enterprise/tokens.toml:/app/tokens.toml:ro \
  --restart unless-stopped \
  facilitator-enterprise:latest
```

### Docker Compose

Create `docker-compose.yml`:

```yaml
version: '3.8'

services:
  facilitator:
    build:
      context: .
      dockerfile: facilitator-enterprise/Dockerfile
    container_name: facilitator
    ports:
      - "8080:8080"
    env_file:
      - facilitator-enterprise/.env
    volumes:
      - ./config.json:/app/config.json:ro
      - ./facilitator-enterprise/config.toml:/app/config.toml:ro
      - ./facilitator-enterprise/hooks.toml:/app/hooks.toml:ro
      - ./facilitator-enterprise/tokens.toml:/app/tokens.toml:ro
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 10s
    logging:
      driver: "json-file"
      options:
        max-size: "10m"
        max-file: "3"
```

Start:
```bash
docker-compose up -d
```

View logs:
```bash
docker-compose logs -f facilitator
```

Stop:
```bash
docker-compose down
```

## Kubernetes Deployment

### Deployment Manifest

Create `deployment.yaml`:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: facilitator
  labels:
    app: facilitator
spec:
  replicas: 2
  selector:
    matchLabels:
      app: facilitator
  template:
    metadata:
      labels:
        app: facilitator
    spec:
      containers:
      - name: facilitator
        image: facilitator-enterprise:latest
        ports:
        - containerPort: 8080
          name: http
        env:
        - name: HOST
          value: "0.0.0.0"
        - name: PORT
          value: "8080"
        - name: API_KEYS
          valueFrom:
            secretKeyRef:
              name: facilitator-secrets
              key: api-keys
        - name: ADMIN_API_KEY
          valueFrom:
            secretKeyRef:
              name: facilitator-secrets
              key: admin-api-key
        envFrom:
        - configMapRef:
            name: facilitator-env
        volumeMounts:
        - name: chain-config
          mountPath: /app/config.json
          subPath: config.json
          readOnly: true
        - name: enterprise-config
          mountPath: /app/config.toml
          subPath: config.toml
          readOnly: true
        livenessProbe:
          httpGet:
            path: /health
            port: 8080
          initialDelaySeconds: 10
          periodSeconds: 30
          timeoutSeconds: 5
        readinessProbe:
          httpGet:
            path: /health
            port: 8080
          initialDelaySeconds: 5
          periodSeconds: 10
          timeoutSeconds: 5
        resources:
          requests:
            memory: "256Mi"
            cpu: "250m"
          limits:
            memory: "512Mi"
            cpu: "500m"
      volumes:
      - name: chain-config
        configMap:
          name: facilitator-chain-config
      - name: enterprise-config
        configMap:
          name: facilitator-enterprise-config
```

### ConfigMaps

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: facilitator-env
data:
  RUST_LOG: "info"
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: facilitator-enterprise-config
data:
  config.toml: |
    [rate_limiting]
    enabled = true
    requests_per_second = 50

    [cors]
    allowed_origins = ["https://app.example.com"]

    [security]
    log_security_events = true
```

### Secrets

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: facilitator-secrets
type: Opaque
stringData:
  api-keys: "key1,key2,key3"
  admin-api-key: "admin-secret"
```

### Service

```yaml
apiVersion: v1
kind: Service
metadata:
  name: facilitator
spec:
  type: ClusterIP
  selector:
    app: facilitator
  ports:
  - port: 8080
    targetPort: 8080
    protocol: TCP
    name: http
```

### Ingress

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: facilitator
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
spec:
  ingressClassName: nginx
  tls:
  - hosts:
    - facilitator.yourdomain.com
    secretName: facilitator-tls
  rules:
  - host: facilitator.yourdomain.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: facilitator
            port:
              number: 8080
```

### Deploy

```bash
# Create namespace
kubectl create namespace facilitator

# Apply manifests
kubectl apply -f deployment.yaml -n facilitator
kubectl apply -f service.yaml -n facilitator
kubectl apply -f ingress.yaml -n facilitator

# Check status
kubectl get pods -n facilitator
kubectl logs -f deployment/facilitator -n facilitator
```

## Reverse Proxy

### Nginx

Create `/etc/nginx/sites-available/facilitator`:

```nginx
upstream facilitator {
    server localhost:8080;
    # For multiple instances
    # server localhost:8081;
    # server localhost:8082;
}

server {
    listen 443 ssl http2;
    server_name facilitator.yourdomain.com;

    ssl_certificate /etc/letsencrypt/live/facilitator.yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/facilitator.yourdomain.com/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers HIGH:!aNULL:!MD5;

    # Security headers
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Frame-Options "SAMEORIGIN" always;
    add_header X-Content-Type-Options "nosniff" always;

    location / {
        proxy_pass http://facilitator;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # Timeouts
        proxy_connect_timeout 60s;
        proxy_send_timeout 60s;
        proxy_read_timeout 60s;

        # Keep-alive
        proxy_http_version 1.1;
        proxy_set_header Connection "";
    }

    # Rate limiting (optional - facilitator has built-in)
    limit_req_zone $binary_remote_addr zone=api_limit:10m rate=10r/s;
    limit_req zone=api_limit burst=20 nodelay;
}

# HTTP to HTTPS redirect
server {
    listen 80;
    server_name facilitator.yourdomain.com;
    return 301 https://$server_name$request_uri;
}
```

Enable and reload:

```bash
sudo ln -s /etc/nginx/sites-available/facilitator /etc/nginx/sites-enabled/
sudo nginx -t
sudo systemctl reload nginx
```

### Caddy

Create `Caddyfile`:

```caddy
facilitator.yourdomain.com {
    reverse_proxy localhost:8080 {
        # Load balancing for multiple instances
        # lb_policy round_robin
        # to localhost:8080 localhost:8081 localhost:8082
    }

    # Security headers
    header {
        Strict-Transport-Security "max-age=31536000; includeSubDomains"
        X-Frame-Options "SAMEORIGIN"
        X-Content-Type-Options "nosniff"
    }

    # Automatic HTTPS via Let's Encrypt
    # No additional SSL configuration needed!
}
```

Start:
```bash
caddy run --config Caddyfile
```

## Systemd Service

Create `/etc/systemd/system/facilitator-enterprise.service`:

```ini
[Unit]
Description=x402 Enterprise Facilitator
After=network.target

[Service]
Type=simple
User=facilitator
Group=facilitator
WorkingDirectory=/opt/facilitator
EnvironmentFile=/opt/facilitator/.env
ExecStart=/opt/facilitator/facilitator-enterprise
Restart=always
RestartSec=5

# Security
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/facilitator/logs

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=facilitator-enterprise

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
# Create user
sudo useradd -r -s /bin/false facilitator

# Set permissions
sudo chown -R facilitator:facilitator /opt/facilitator

# Enable service
sudo systemctl daemon-reload
sudo systemctl enable facilitator-enterprise
sudo systemctl start facilitator-enterprise

# Check status
sudo systemctl status facilitator-enterprise

# View logs
sudo journalctl -u facilitator-enterprise -f
```

## SSL/TLS Certificates

### Let's Encrypt with Certbot

```bash
# Install certbot
sudo apt install certbot python3-certbot-nginx

# Obtain certificate
sudo certbot --nginx -d facilitator.yourdomain.com

# Auto-renewal (already configured by default)
sudo systemctl status certbot.timer
```

## Zero-Downtime Deployment

### Rolling Updates (Kubernetes)

```bash
# Update image
kubectl set image deployment/facilitator \
  facilitator=facilitator-enterprise:v2 \
  -n facilitator

# Monitor rollout
kubectl rollout status deployment/facilitator -n facilitator

# Rollback if needed
kubectl rollout undo deployment/facilitator -n facilitator
```

### Blue-Green Deployment

```bash
# Deploy new version to port 8081
docker run -d -p 8081:8080 --name facilitator-green \
  --env-file .env facilitator-enterprise:v2

# Test green deployment
curl http://localhost:8081/health

# Update nginx upstream to point to 8081
# Reload nginx
sudo systemctl reload nginx

# Stop old version
docker stop facilitator-blue
```

## Monitoring and Alerts

### Quick Health Check

```bash
# Check service health
curl https://facilitator.yourdomain.com/health

# Check from monitoring service (cron job)
0 * * * * curl -f https://facilitator.yourdomain.com/health || send_alert
```

### OpenTelemetry

Configure OpenTelemetry for distributed tracing and metrics:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=https://api.honeycomb.io:443
OTEL_EXPORTER_OTLP_HEADERS=x-honeycomb-team=YOUR_API_KEY
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
```

## Backup and Disaster Recovery

### Configuration Backup

```bash
# Backup configuration
tar czf facilitator-backup-$(date +%Y%m%d).tar.gz \
  .env config.json config.toml hooks.toml tokens.toml

# Store in S3
aws s3 cp facilitator-backup-*.tar.gz s3://backups/facilitator/
```

### Disaster Recovery Plan

1. **Service Down**: Auto-restart via systemd/Kubernetes
2. **Data Loss**: Restore from configuration backup
3. **Infrastructure Failure**: Redeploy to new infrastructure using backups
4. **Key Compromise**: Rotate keys, update secrets, redeploy

## Scaling

### Horizontal Scaling

**Kubernetes:**
```bash
# Scale to 5 replicas
kubectl scale deployment/facilitator --replicas=5 -n facilitator

# Auto-scaling
kubectl autoscale deployment/facilitator \
  --cpu-percent=70 --min=2 --max=10 -n facilitator
```

### Multi-Wallet Configuration

Provide multiple signer wallets in `config.json` for concurrent transaction submission:

```json
{
  "chains": {
    "eip155:8453": {
      "signers": ["0xKey1", "0xKey2", "0xKey3", "0xKey4", "0xKey5"],
      "rpc": [{"http": "https://rpc.example.com"}]
    }
  }
}
```

Multiple wallets enable parallel settlement using round-robin selection:
- N wallets = N concurrent settlements
- 5 wallets ~ 25 TPS, 20 wallets ~ 100 TPS

### Multi-Region Deployment

Deploy facilitator instances in multiple regions with:
- Regional RPC endpoints
- Regional secret storage
- Global load balancer (Cloudflare, AWS Global Accelerator)

## Security Hardening

- Run as non-root user
- Use read-only root filesystem where possible
- Restrict network access (firewall rules)
- Enable container security scanning
- Use secret rotation
- Implement audit logging
- Regular security updates

## Troubleshooting

### Common Issues

**Service won't start:**
- Check logs: `docker logs facilitator` or `journalctl -u facilitator-enterprise`
- Verify environment variables
- Check RPC endpoint connectivity
- Verify `config.json` is valid JSON

**502 Bad Gateway (Nginx):**
- Verify facilitator is running: `curl localhost:8080/health`
- Check Nginx error logs: `tail -f /var/log/nginx/error.log`

**High memory usage:**
- Increase resources or reduce traffic
- Check for memory leaks in logs
- Monitor with htop/prometheus

## Further Reading

- [Configuration Guide](CONFIGURATION.md) - Configuration options
- [Security Documentation](SECURITY.md) - Security best practices
- [Batch Settlement Guide](BATCH_SETTLEMENT.md) - High-throughput configuration
- [API Reference](API.md) - Endpoint documentation
