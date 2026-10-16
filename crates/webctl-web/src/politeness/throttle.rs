//! 每主机并发上限与最小请求间隔（PLAN v0.4 §3.8）。
//!
//! 设计：**先抢并发许可，再预约时间片**——
//! 1. 先取该主机的并发信号量（上限 `max_concurrency_per_host`）；
//! 2. 拿到许可后，才按 `min_interval` 预约自己的开始时刻并等待到点。
//!
//! 顺序不能颠倒：如果先预约再排队等许可，请求在信号量上等待超过自己的预约时刻后，
//! 会与后到者"撞车"同时开始（实测：并发 2、间隔 200ms 时两次开始相差 7.9µs）。
//! 先拿许可再预约，才能保证**实际开始时刻**之间始终满足最小间隔。
//! 不同主机互不影响。
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

    /// 取得一次请求许可：**先占用并发名额，再等到该主机允许的开始时刻**。
    ///
    /// 返回的许可在 drop 时释放并发名额。
    pub async fn acquire(&self, host: &str) -> HostPermit {
        let slot = self.slot(host).await;
        // 必须先拿许可：否则排队等待期间预约时刻已过，多个请求会同时开始，最小间隔形同虚设。
        let permit = Arc::clone(&slot.semaphore)
            .acquire_owned()
            .await
            .expect("信号量不会关闭");
        // 拿到许可之后才预约开始时刻，保证"真正开始"之间满足间隔。
        let start = {
            let mut next = slot.next_start.lock().await;
            let now = Instant::now();
            let start = if *next > now { *next } else { now };
            *next = start + self.config.min_interval;
            start
        };
        sleep_until(start).await;
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

        let mut starts = starts.lock().await.clone();
        starts.sort();
        // 单点间隔给 25% 余量（负载下先到者可能被推迟唤醒，压缩记录间隔）；
        // 真正抖动免疫的不变量是"总跨度"：6 次请求、间隔 60ms ⇒ 跨度至少 ≈300ms。
        for pair in starts.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(
                gap >= Duration::from_millis(45),
                "相邻请求间隔 {gap:?} 明显小于最小间隔"
            );
        }
        let span = starts.last().expect("starts").duration_since(starts[0]);
        // 余量 20%：windows CI 上实测过 288.4ms（名义 300ms）——定时器粒度 + 调度延迟会压缩跨度。
        // 真正要抓的缺陷（预约顺序倒置）会产生**微秒级**跨度，20% 余量照样抓得住。
        assert!(
            span >= Duration::from_millis(240),
            "总跨度 {span:?} 不足（6 次 × 60ms 间隔名义 300ms）"
        );
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

    /// 回归（审阅者探针）：**在信号量上排队的请求也必须保持最小间隔**。
    ///
    /// 旧实现"先预约再排队"时，排队超过预约时刻的请求会与后到者同时开始
    /// （实测两次开始相差 7.9µs，而 min_interval 是 200ms）。
    #[tokio::test]
    async fn queued_requests_still_keep_min_interval() {
        let gate = Arc::new(HostGate::new(ThrottleConfig {
            max_concurrency_per_host: 2,
            min_interval: Duration::from_millis(200),
        }));
        let starts = Arc::new(tokio::sync::Mutex::new(Vec::<Instant>::new()));

        // 两个"长占用"任务先把两个许可占住，逼迫后续请求排队。
        let mut holders = Vec::new();
        for hold in [Duration::from_millis(500), Duration::from_millis(700)] {
            let gate = Arc::clone(&gate);
            let starts = Arc::clone(&starts);
            holders.push(tokio::spawn(async move {
                let permit = gate.acquire("queued.test").await;
                starts.lock().await.push(Instant::now());
                tokio::time::sleep(hold).await;
                drop(permit);
            }));
        }
        tokio::time::sleep(Duration::from_millis(30)).await;

        let mut waiters = Vec::new();
        for _ in 0..2 {
            let gate = Arc::clone(&gate);
            let starts = Arc::clone(&starts);
            waiters.push(tokio::spawn(async move {
                let permit = gate.acquire("queued.test").await;
                starts.lock().await.push(Instant::now());
                drop(permit);
            }));
        }
        for task in holders.into_iter().chain(waiters) {
            task.await.expect("task");
        }

        let mut stamps = starts.lock().await.clone();
        stamps.sort();
        assert_eq!(stamps.len(), 4);
        for pair in stamps.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(
                gap >= Duration::from_millis(150),
                "排队场景下相邻开始间隔只有 {gap:?}（应 ≈200ms）"
            );
        }
        let span = stamps[3].duration_since(stamps[0]);
        // 同上：名义 600ms，留 20% 余量（抖动只会让跨度变大，压缩只会来自"预约撞车"这类缺陷）。
        assert!(
            span >= Duration::from_millis(480),
            "排队场景总跨度只有 {span:?}（4 次 × 200ms 间隔名义 600ms）"
        );
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
            started.elapsed() >= Duration::from_millis(60),
            "第二次请求等待 {:?}",
            started.elapsed()
        );
        drop((first, second));
    }
}
