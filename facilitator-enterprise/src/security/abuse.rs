use axum::{
    body::Body,
    extract::{ConnectInfo, Request},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Clone)]
pub struct AbuseDetector {
    config: Arc<AbuseDetectorConfig>,
    invalid_signatures: Arc<DashMap<IpAddr, InvalidSignatureTracker>>,
}

#[derive(Debug, Clone)]
pub struct AbuseDetectorConfig {
    pub enabled: bool,
    pub invalid_signature_threshold: u32,
    pub tracking_window: Duration,
    pub log_events: bool,
}

impl Default for AbuseDetectorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            invalid_signature_threshold: 10,
            tracking_window: Duration::from_secs(300),
            log_events: true,
        }
    }
}

#[derive(Debug, Clone)]
struct InvalidSignatureTracker {
    count: u32,
    first_seen: SystemTime,
    last_seen: SystemTime,
}

impl AbuseDetector {
    pub fn new(config: AbuseDetectorConfig) -> Self {
        Self {
            config: Arc::new(config),
            invalid_signatures: Arc::new(DashMap::new()),
        }
    }

    #[allow(dead_code)] // Called by verify/settle handlers when signature validation fails
    pub fn record_invalid_signature(&self, ip: IpAddr) {
        if !self.config.enabled {
            return;
        }

        let now = SystemTime::now();
        self.invalid_signatures
            .entry(ip)
            .and_modify(|tracker| {
                if now.duration_since(tracker.first_seen).unwrap_or_default()
                    > self.config.tracking_window
                {
                    tracker.count = 1;
                    tracker.first_seen = now;
                    tracker.last_seen = now;
                } else {
                    tracker.count += 1;
                    tracker.last_seen = now;

                    if tracker.count == self.config.invalid_signature_threshold
                        && self.config.log_events
                    {
                        tracing::warn!(
                            ip = %ip,
                            count = tracker.count,
                            "Suspicious activity: repeated invalid signatures detected"
                        );
                    }
                }
            })
            .or_insert_with(|| InvalidSignatureTracker {
                count: 1,
                first_seen: now,
                last_seen: now,
            });
    }

    pub fn is_suspicious(&self, ip: &IpAddr) -> bool {
        if !self.config.enabled {
            return false;
        }

        if let Some(tracker) = self.invalid_signatures.get(ip) {
            let now = SystemTime::now();
            if now.duration_since(tracker.first_seen).unwrap_or_default()
                <= self.config.tracking_window
            {
                return tracker.count >= self.config.invalid_signature_threshold;
            }
        }
        false
    }

    pub fn record_malformed_payload(&self, ip: IpAddr, error: &str) {
        if !self.config.enabled {
            return;
        }

        if self.config.log_events {
            tracing::debug!(
                ip = %ip,
                error = error,
                "Malformed payload received"
            );
        }
    }

    pub fn cleanup_old_data(&self) {
        let now = SystemTime::now();
        let tracking_window = self.config.tracking_window;

        self.invalid_signatures.retain(|_, tracker| {
            now.duration_since(tracker.last_seen).unwrap_or_default() <= tracking_window
        });
    }

    pub fn get_stats(&self) -> AbuseStats {
        let total_ips_tracked = self.invalid_signatures.len();
        let suspicious_ips = self
            .invalid_signatures
            .iter()
            .filter(|entry| {
                let now = SystemTime::now();
                let tracker = entry.value();
                now.duration_since(tracker.first_seen).unwrap_or_default()
                    <= self.config.tracking_window
                    && tracker.count >= self.config.invalid_signature_threshold
            })
            .count();

        AbuseStats {
            total_ips_tracked,
            suspicious_ips,
        }
    }

    pub async fn middleware(&self, req: Request, next: Next) -> Response {
        if !self.config.enabled {
            return next.run(req).await;
        }

        let ip = extract_ip_from_request(&req);

        if let Some(ip_addr) = &ip {
            if self.is_suspicious(ip_addr) {
                tracing::warn!(ip = %ip_addr, "Blocking request from suspicious IP");
                return (StatusCode::FORBIDDEN, "Too many invalid requests").into_response();
            }
        }

        let response = next.run(req).await;

        if let Some(ip_addr) = ip {
            if response.status() == StatusCode::BAD_REQUEST {
                self.record_malformed_payload(ip_addr, "Bad request");
            }
        }

        response
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
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

#[derive(Debug, Clone)]
pub struct AbuseStats {
    pub total_ips_tracked: usize,
    pub suspicious_ips: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_signature_tracking() {
        let config = AbuseDetectorConfig {
            enabled: true,
            invalid_signature_threshold: 3,
            tracking_window: Duration::from_secs(60),
            log_events: false,
        };

        let detector = AbuseDetector::new(config);
        let ip: IpAddr = "192.168.1.1".parse().unwrap();

        detector.record_invalid_signature(ip);
        detector.record_invalid_signature(ip);
        assert!(!detector.is_suspicious(&ip));

        detector.record_invalid_signature(ip);
        assert!(detector.is_suspicious(&ip));
    }

    #[test]
    fn test_tracking_window_reset() {
        let config = AbuseDetectorConfig {
            enabled: true,
            invalid_signature_threshold: 5,
            tracking_window: Duration::from_millis(100),
            log_events: false,
        };

        let detector = AbuseDetector::new(config);
        let ip: IpAddr = "192.168.1.1".parse().unwrap();

        detector.record_invalid_signature(ip);
        assert_eq!(detector.invalid_signatures.get(&ip).unwrap().count, 1);

        std::thread::sleep(Duration::from_millis(150));

        detector.record_invalid_signature(ip);
        assert_eq!(detector.invalid_signatures.get(&ip).unwrap().count, 1);
    }

    #[test]
    fn test_get_stats() {
        let config = AbuseDetectorConfig {
            enabled: true,
            invalid_signature_threshold: 2,
            tracking_window: Duration::from_secs(60),
            log_events: false,
        };

        let detector = AbuseDetector::new(config);
        let ip1: IpAddr = "192.168.1.1".parse().unwrap();
        let ip2: IpAddr = "192.168.1.2".parse().unwrap();

        detector.record_invalid_signature(ip1);
        detector.record_invalid_signature(ip1);
        detector.record_invalid_signature(ip1);

        detector.record_invalid_signature(ip2);

        let stats = detector.get_stats();
        assert_eq!(stats.total_ips_tracked, 2);
        assert_eq!(stats.suspicious_ips, 1);
    }
}
