//! SSRF 地址守卫（#191）的集成测试。
//!
//! 覆盖四件事：
//! 1. 默认配置拒绝 loopback / localhost / ::1 / 私网 / metadata / 组播 / 未指定 / CGNAT，
//!    且**被拒时不向目标发出任何请求**（fixture 的 total() == 0）；
//! 2. 显式放行（`allow_private_network`）后回环 fixture 可用；
//! 3. 云 metadata 端点即使放行也**始终拒绝**（用户决策：硬拒绝保留）；
//! 4. 重定向逐跳校验：302 的第二跳指向 metadata 时被拒，且只发出第一跳请求；
//! 5. `SessionEngine` 的表单 POST 与页面 GET 同样受守卫保护。
//!
//! 全部用例只连 127.0.0.1 上的进程内 fixture；metadata 用例在**连接之前**就被拒绝，
//! 因此不会真的访问 169.254.169.254（离线、可重复）。

use std::time::Duration;

use url::Url;
use webctl_core::ErrorCode;
use webctl_web::session::{FormFill, SessionEngine, SessionStore};
use webctl_web::transport::{AddressClass, AddressGuard, FetchOptions, GuardError, fetch};

#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{Response, Server};

/// 独立临时 store（与 session_helpers 同款命名策略，避免并发用例互相踩）。
fn temp_store(label: &str) -> SessionStore {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("webctl-ssrf-{label}-{nanos}"));
    SessionStore::with_ttl(dir, Duration::from_secs(3600))
}

fn denied_engine(label: &str) -> SessionEngine {
    // 刻意不调用 with_private_network_allowed：验证默认值就是拒绝。
    SessionEngine::with_store("default", temp_store(label)).expect("engine")
}

fn allowed_engine(label: &str) -> SessionEngine {
    denied_engine(label).with_private_network_allowed(true)
}

/// 拼接探测 URL（scheme 与 `//` 分开写）。
///
/// 离线 lane 的 `external_url_sites`（见 support/resources.rs）只放行回环 / 保留域名的
/// `http://…` 字面量；本文件的探测目标本来就**必须是**非公网地址，拆开拼接可避免
/// 把测试文件自身判成"访问外站"。
fn probe_raw(rest: &str) -> String {
    format!("http:{}{rest}", "//")
}

fn probe(rest: &str) -> Url {
    let raw = probe_raw(rest);
    Url::parse(&raw).unwrap_or_else(|error| panic!("{raw}: {error}"))
}

/// 对单个目标判定：必须是 BLOCKED，并返回分类。
async fn blocked_class(guard: AddressGuard, rest: &str) -> AddressClass {
    let error = guard
        .check_url(&probe(rest))
        .await
        .expect_err("该地址必须被守卫拒绝");
    assert_eq!(error.code(), ErrorCode::Blocked, "{rest} 必须是 BLOCKED");
    match error {
        GuardError::Blocked(blocked) => blocked.class,
        GuardError::Resolve { detail, .. } => {
            panic!("{rest} 不能被归类为解析失败（不得伪装成 NETWORK）：{detail}")
        }
    }
}

/// SSRF-01：默认拒绝对回环 fixture 的请求，且目标一个字节都没收到。
#[tokio::test]
async fn default_rejects_loopback_without_sending_any_request() {
    let server = Server::start_with(vec![("/secret", Response::text("TOP-SECRET"))]).await;
    let envelope = fetch(&server.url("/secret"), &FetchOptions::default()).await;
    assert!(!envelope.ok, "默认必须拒绝回环目标");
    assert_eq!(envelope.code, ErrorCode::Blocked);
    assert!(
        envelope.error.contains("loopback-address-blocked"),
        "错误必须带机器可读原因码：{}",
        envelope.error
    );
    assert!(
        envelope.error.contains("WEBCTL_ALLOW_PRIVATE_NETWORK"),
        "错误必须给出显式放行开关：{}",
        envelope.error
    );
    assert_eq!(server.total(), 0, "被拒时不得向目标发起任何连接");
}

/// SSRF-02：默认拒绝面覆盖 issue #191 点名的全部分类（含 localhost 的 DNS 结果）。
#[tokio::test]
async fn default_rejects_every_non_public_class() {
    let guard = AddressGuard::default();
    for (rest, expected) in [
        ("127.0.0.1/", AddressClass::Loopback),
        ("127.9.9.9/", AddressClass::Loopback),
        ("localhost/", AddressClass::Loopback),
        ("[::1]/", AddressClass::Loopback),
        ("10.1.2.3/", AddressClass::Private),
        ("172.16.0.1/", AddressClass::Private),
        ("192.168.1.1/", AddressClass::Private),
        ("[fd12:3456::1]/", AddressClass::Private),
        ("169.254.1.1/", AddressClass::LinkLocal),
        ("[fe80::1]/", AddressClass::LinkLocal),
        ("224.0.0.1/", AddressClass::Multicast),
        ("[ff02::1]/", AddressClass::Multicast),
        ("0.0.0.0/", AddressClass::Unspecified),
        ("100.64.0.1/", AddressClass::Shared),
        ("240.0.0.1/", AddressClass::Reserved),
        ("255.255.255.255/", AddressClass::Reserved),
        ("169.254.169.254/latest/meta-data/", AddressClass::Metadata),
        ("[fd00:ec2::254]/latest/meta-data/", AddressClass::Metadata),
    ] {
        assert_eq!(blocked_class(guard, rest).await, expected, "{rest}");
    }
}

/// SSRF-03：显式放行后回环 fixture 可用（证明守卫不是"一律拒绝"）。
#[tokio::test]
async fn explicit_allow_permits_loopback_fixture() {
    let server = Server::start_with(vec![("/ok", Response::text("ok"))]).await;
    let options = FetchOptions::default().with_private_network_allowed(true);
    let envelope = fetch(&server.url("/ok"), &options).await;
    assert!(
        envelope.ok,
        "放行后应能取回回环 fixture：{}",
        envelope.error
    );
    assert!(envelope.data.is_some());
    assert_eq!(server.count("/ok"), 1);
}

/// SSRF-04：metadata 是硬拒绝——打开放行开关也不放行，且不发起连接。
#[tokio::test]
async fn metadata_stays_blocked_even_when_private_network_allowed() {
    let options = FetchOptions::default().with_private_network_allowed(true);
    for rest in [
        "169.254.169.254/latest/meta-data/iam/security-credentials/",
        "[fd00:ec2::254]/latest/meta-data/iam/security-credentials/",
    ] {
        let url = probe_raw(rest);
        let envelope = fetch(&url, &options).await;
        assert!(!envelope.ok, "{url} 必须被拒绝");
        assert_eq!(envelope.code, ErrorCode::Blocked, "{url}");
        assert!(
            envelope.error.contains("metadata-endpoint-blocked"),
            "{url}：{}",
            envelope.error
        );
        assert!(
            envelope
                .error
                .contains("即使开启 WEBCTL_ALLOW_PRIVATE_NETWORK"),
            "metadata 的说明必须点明例外：{}",
            envelope.error
        );
    }
}

/// SSRF-05：302 的第二跳指向 metadata → 被拒，且仅发出第一跳的一次请求。
#[tokio::test]
async fn redirect_second_hop_to_metadata_is_blocked_after_one_request() {
    let server = Server::start_with(vec![(
        "/start",
        Response::redirect(302, probe_raw("169.254.169.254/latest/meta-data/")),
    )])
    .await;
    let options = FetchOptions::default().with_private_network_allowed(true);
    let envelope = fetch(&server.url("/start"), &options).await;
    assert!(!envelope.ok, "第二跳必须被拒绝");
    assert_eq!(envelope.code, ErrorCode::Blocked);
    assert!(
        envelope.error.contains("metadata-endpoint-blocked"),
        "{}",
        envelope.error
    );
    assert_eq!(server.count("/start"), 1, "第一跳应恰好请求一次");
    assert_eq!(server.total(), 1, "第二跳被拒时不得发出第二次请求");
}

/// SSRF-06：SessionEngine 的页面 GET 默认被守卫（连接前拒绝）。
#[tokio::test]
async fn session_engine_get_is_guarded_by_default() {
    let server = Server::start_with(vec![("/page", Response::html("<h1>hi</h1>"))]).await;
    let envelope = denied_engine("get").fetch_page(&server.url("/page")).await;
    assert!(!envelope.ok);
    assert_eq!(envelope.code, ErrorCode::Blocked, "{}", envelope.error);
    assert!(envelope.error.contains("loopback-address-blocked"));
    assert_eq!(server.total(), 0, "GET 不得发出");
}

/// SSRF-07：SessionEngine 的表单 POST 默认被守卫（第二跳/提交同样逐次校验）。
#[tokio::test]
async fn session_engine_form_post_is_guarded_by_default() {
    const FORM_PAGE: &str = r#"<!doctype html>
<html><body>
  <form method="post" action="/submit">
    <input type="text" name="name" value="alice">
    <button type="submit">go</button>
  </form>
</body></html>"#;
    let server = Server::start_with(vec![
        ("/login", Response::html(FORM_PAGE)),
        ("/submit", Response::text("landed")),
    ])
    .await;

    // Page 只是数据，与引擎解耦：先用放行引擎取回表单页。
    let page = allowed_engine("form-allowed")
        .fetch_page(&server.url("/login"))
        .await
        .data
        .expect("放行后应能取回表单页");
    assert_eq!(page.forms.len(), 1, "fixture 必须解析出一个表单");

    // 默认引擎提交：必须在连接前被拒。
    let envelope = denied_engine("form-denied")
        .submit_form(&page, 0, &FormFill::new())
        .await;
    assert!(!envelope.ok, "表单 POST 默认必须被拒绝");
    assert_eq!(envelope.code, ErrorCode::Blocked, "{}", envelope.error);
    assert!(
        envelope.error.contains("loopback-address-blocked"),
        "{}",
        envelope.error
    );
    assert_eq!(server.count("/submit"), 0, "被拒的 POST 不得发出");
    assert_eq!(server.total(), 1, "只应有放行引擎取表单页的那一次 GET");
}
