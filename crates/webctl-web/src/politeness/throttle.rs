//! 每主机并发上限与最小请求间隔（PLAN v0.4 §3.8）。
//!
//! 设计：**先预约时间片，再取并发许可**——
//! 1. 同一主机的请求按 `min_interval` 预约各自的开始时刻（互不重叠）；
//! 2. 预约到点后再去抢该主机的并发信号量（上限 `max_concurrency_per_host`）。
//!
//! 这样既能保证请求间隔，又不会把并发压成 1；不同主机互不影响。
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, sleep_until};

/// 限速配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThrottleConfig {
    /// 每主机并发上限。
    pub max_concurrency_per_host: usize,
    /// 同主机最小请求间隔。
    pub min_interval: Duration,
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        Self {
            max_concurrency_per_host: 2,
            min_interval: Duration::from_secs(1),
        }
    }
}

/// 单主机的调度状态。
struct HostSlot {
    semaphore: Arc<Semaphore>,
    next_start: Mutex<Instant>,
}

/// 全局限速门。
pub struct HostGate {
    config: ThrottleConfig,
    hosts: Mutex<HashMap<String, Arc<HostSlot>>>,
}

impl HostGate {
    /// 新建。
    pub fn new(config: ThrottleConfig) -> Self {
        Self {
            config: ThrottleConfig {
                max_concurrency_per_host: config.max_concurrency_per_host.max(1),
                min_interval: config.min_interval,
            },
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// 配置。
    pub fn config(&self) -> &ThrottleConfig {
        &self.config
    }

    async fn slot(&self, host: &str) -> Arc<HostSlot> {
        let mut hosts = self.hosts.lock().await;
        hosts
            .entry(host.to_owned())
            .or_insert_with(|| {
                Arc::new(HostSlot {
                    semaphore: Arc::new(Semaphore::new(self.config.max_concurrency_per_host)),
                    next_start: Mutex::new(Instant::now()),
                })
            })
            .clone()
    }

    /// 取得一次请求许可：内部会先等到该主机允许的开始时刻，再占用并发名额。
    ///
    /// 返回的许可在 drop 时释放并发名额。
    pub async fn acquire(&self, host: &str) -> HostPermit {
        let slot = self.slot(host).await;
        let start = {
            let mut next = slot.next_start.lock().await;
            let now = Instant::now();
            let start = if *next > now { *next } else { now };
            *next = start + self.config.min_interval;
            start
        };
        sleep_until(start).await;
        let permit = Arc::clone(&slot.semaphore)
            .acquire_owned()
            .await
            .expect("信号量不会关闭");
        HostPermit {
            _permit: permit,
            host: host.to_owned(),
        }
    }
}

/// 一次请求的许可（drop 即释放）。
pub struct HostPermit {
    _permit: OwnedSemaphorePermit,
    /// 所属主机。
    pub host: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// POLITE-01 / POLITE-02：同主机并发 ≤ 2，且请求间隔 ≥ min_interval。
    #[tokio::test]
    async fn enforces_concurrency_and_interval_per_host() {
        let gate = Arc::new(HostGate::new(ThrottleConfig {
            max_concurrency_per_host: 2,
            min_interval: Duration::from_millis(60),
        }));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(tokio::sync::Mutex::new(Vec::<Instant>::new()));

        let mut tasks = Vec::new();
        for _ in 0..6 {
            let gate = Arc::clone(&gate);
            let in_flight = Arc::clone(&in_flight);
            let peak = Arc::clone(&peak);
            let starts = Arc::clone(&starts);
            tasks.push(tokio::spawn(async move {
                let permit = gate.acquire("example.test").await;
                starts.lock().await.push(Instant::now());
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(40)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                drop(permit);
            }));
        }
        for task in tasks {
            task.await.expect("task");
        }

        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "峰值并发 {}",
            peak.load(Ordering::SeqCst)
        );

        let starts = starts.lock().await.clone();
        for pair in starts.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(
                gap >= Duration::from_millis(55),
                "相邻请求间隔 {gap:?} 小于最小间隔"
            );
        }
    }

    /// 不同主机互不阻塞。
    #[tokio::test]
    async fn different_hosts_do_not_wait_for_each_other() {
        let gate = HostGate::new(ThrottleConfig {
            max_concurrency_per_host: 1,
            min_interval: Duration::from_secs(30),
        });
        let started = Instant::now();
        let a = gate.acquire("a.test").await;
        let b = gate.acquire("b.test").await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "不同主机不应互相等待：{:?}",
            started.elapsed()
        );
        drop((a, b));
    }

    /// 同主机第二次请求必须等到最小间隔之后。
    #[tokio::test]
    async fn second_request_waits_for_interval() {
        let gate = HostGate::new(ThrottleConfig {
            max_concurrency_per_host: 4,
            min_interval: Duration::from_millis(80),
        });
        let first = gate.acquire("c.test").await;
        let started = Instant::now();
        let second = gate.acquire("c.test").await;
        assert!(
            started.elapsed() >= Duration::from_millis(70),
            "第二次请求等待 {:?}",
            started.elapsed()
        );
        drop((first, second));
    }
}
