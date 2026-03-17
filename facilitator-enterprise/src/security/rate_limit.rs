use axum::{
    body::Body,
    extract::{ConnectInfo, Request},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Clone)]
pub struct RateLimiter {
    config: Arc<RateLimiterConfig>,
    violations: Arc<DashMap<IpAddr, ViolationTracker>>,
    bans: Arc<DashMap<IpAddr, SystemTime>>,
    request_history: Arc<DashMap<IpAddr, Vec<SystemTime>>>,
}

#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    pub enabled: bool,
    pub requests_per_second: u32,
    pub ban_duration: Duration,
    pub ban_threshold: u32,
    pub whitelisted_ips: Vec<ipnetwork::IpNetwork>,
}

#[derive(Debug, Clone)]
struct ViolationTracker {
    count: u32,
    last_reset: SystemTime,
}

impl RateLimiter {
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config: Arc::new(config),
            violations: Arc::new(DashMap::new()),
            bans: Arc::new(DashMap::new()),
            request_history: Arc::new(DashMap::new()),
        }
    }

    pub async fn middleware(&self, req: Request, next: Next) -> Response {
        if !self.config.enabled {
            return next.run(req).await;
        }

        let ip = match extract_ip_from_request(&req) {
            Some(ip) => ip,
            None => {
                tracing::warn!("Could not extract IP from request");
                return next.run(req).await;
            }
        };

        if self.is_whitelisted(&ip) {
            return next.run(req).await;
        }

        if self.is_banned(&ip) {
            tracing::warn!(ip = %ip, "Request blocked: IP is temporarily banned");
            return (StatusCode::TOO_MANY_REQUESTS, "IP temporarily banned").into_response();
        }

        if self.should_rate_limit(&ip) {
            self.record_violation(&ip);

            tracing::warn!(ip = %ip, "Rate limit exceeded");

            if self.should_ban(&ip) {
                self.ban_ip(&ip);
                tracing::warn!(
                    ip = %ip,
                    ban_duration_secs = self.config.ban_duration.as_secs(),
                    "IP banned due to repeated violations"
                );
            }

            return (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response();
        }

        next.run(req).await
    }

    fn is_banned(&self, ip: &IpAddr) -> bool {
        if let Some(entry) = self.bans.get(ip) {
            let ban_expiry = *entry.value();
            if SystemTime::now() < ban_expiry {
                return true;
            } else {
                drop(entry);
                self.bans.remove(ip);
                self.violations.remove(ip);
            }
        }
        false
    }

    fn is_whitelisted(&self, ip: &IpAddr) -> bool {
        self.config
            .whitelisted_ips
            .iter()
            .any(|network| network.contains(*ip))
    }

    fn should_rate_limit(&self, ip: &IpAddr) -> bool {
        let now = SystemTime::now();
        let window = Duration::from_secs(1);
        let max_requests = self.config.requests_per_second as usize;

        let mut history = self.request_history.entry(*ip).or_insert_with(Vec::new);

        history
            .value_mut()
            .retain(|&timestamp| now.duration_since(timestamp).unwrap_or_default() < window);

        if history.value().len() >= max_requests {
            return true;
        }

        history.value_mut().push(now);
        false
    }

    fn record_violation(&self, ip: &IpAddr) {
        let now = SystemTime::now();
        self.violations
            .entry(*ip)
            .and_modify(|tracker| {
                if now.duration_since(tracker.last_reset).unwrap_or_default()
                    > Duration::from_secs(60)
                {
                    tracker.count = 1;
                    tracker.last_reset = now;
                } else {
                    tracker.count += 1;
                }
            })
            .or_insert_with(|| ViolationTracker {
                count: 1,
                last_reset: now,
            });
    }

    fn should_ban(&self, ip: &IpAddr) -> bool {
        if let Some(tracker) = self.violations.get(ip) {
            tracker.count >= self.config.ban_threshold
        } else {
            false
        }
    }

    fn ban_ip(&self, ip: &IpAddr) {
        let ban_until = SystemTime::now() + self.config.ban_duration;
        self.bans.insert(*ip, ban_until);
    }

    pub fn cleanup_expired_bans(&self) {
        let now = SystemTime::now();
        let window = Duration::from_secs(1);

        self.bans.retain(|_, &mut expiry| now < expiry);

        // Clean up stale request history and violation trackers to prevent
        // unbounded memory growth from IP-rotating attackers.
        self.request_history.retain(|_, timestamps| {
            timestamps.retain(|&ts| now.duration_since(ts).unwrap_or_default() < window);
            !timestamps.is_empty()
        });

        self.violations.retain(|_, tracker| {
            now.duration_since(tracker.last_reset).unwrap_or_default() < Duration::from_secs(60)
        });
    }
}

fn extract_ip_from_request(req: &Request<Body>) -> Option<IpAddr> {
    if let Some(forwarded_for) = req.headers().get("x-forwarded-for") {
        if let Ok(value) = forwarded_for.to_str() {
            if let Some(ip_str) = value.split(',').next() {
                if let Ok(ip) = ip_str.trim().parse() {
                    return Some(ip);
                }
            }
        }
    }

    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(value) = real_ip.to_str() {
            if let Ok(ip) = value.parse() {
                return Some(ip);
            }
        }
    }

    req.extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_creation() {
        let config = RateLimiterConfig {
            enabled: true,
            requests_per_second: 10,
            ban_duration: Duration::from_secs(300),
            ban_threshold: 5,
            whitelisted_ips: vec![],
        };
        let limiter = RateLimiter::new(config);
        assert!(limiter.config.enabled);
    }

    #[test]
    fn test_violation_tracking() {
        let config = RateLimiterConfig {
            enabled: true,
            requests_per_second: 10,
            ban_duration: Duration::from_secs(300),
            ban_threshold: 3,
            whitelisted_ips: vec![],
        };
        let limiter = RateLimiter::new(config);
        let ip: IpAddr = "192.168.1.1".parse().unwrap();

        limiter.record_violation(&ip);
        limiter.record_violation(&ip);
        limiter.record_violation(&ip);

        assert!(limiter.should_ban(&ip));
    }

    #[test]
    fn test_ban_expiry() {
        let config = RateLimiterConfig {
            enabled: true,
            requests_per_second: 10,
            ban_duration: Duration::from_millis(100),
            ban_threshold: 1,
            whitelisted_ips: vec![],
        };
        let limiter = RateLimiter::new(config);
        let ip: IpAddr = "192.168.1.1".parse().unwrap();

        limiter.ban_ip(&ip);
        assert!(limiter.is_banned(&ip));

        std::thread::sleep(Duration::from_millis(150));
        assert!(!limiter.is_banned(&ip));
    }
}
