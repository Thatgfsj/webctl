//! SESS-01…15 的离线集成测试（PLAN v0.4 §2「会话」域，PR 04）。
//!
//! 全部用例只访问 127.0.0.1 上的本地表单站，不访问外网、不启动子进程。
//! fixture 与辅助在 tests/support/session_site.rs / session_helpers.rs（两个二进制共用）。

use webctl_core::ErrorCode;
use webctl_web::session::{FileUpload, FormFill, SessionEngine, validate_session_name};

// ---------------------------------------------------------------------------
// 本地表单站 fixture（手写最小 HTTP/1.1，支持 GET/POST/cookie/重定向/multipart）

#[path = "support/session_helpers.rs"]
mod helpers;
#[path = "support/session_site.rs"]
mod site;

use helpers::{credentials, engine, temp_store};
use site::Site;

#[tokio::test]
async fn sess_01_login_success() {
    let site = Site::start().await;
    let engine = engine("sess01");
    let env = engine
        .login(&site.url("/login"), None, &credentials())
        .await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    assert_eq!(env.validate(), Ok(()));
    let page = env.data.expect("page");
    assert_eq!(page.status, 200);
    assert_eq!(page.url, site.url("/dashboard"));
    assert!(page.html.contains("Welcome alice"), "{}", page.html);
    assert!(engine.is_logged_in());

    let protected = engine.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(protected.ok, "{}", protected.error);
    assert!(protected.data.expect("data").html.contains("Welcome alice"));
}

/// SESS-02：未登录访问受保护页面 → SESSION_LOST，且与其它失败码可区分。
#[tokio::test]
async fn sess_02_not_logged_in_is_session_lost() {
    let site = Site::start().await;
    let engine = engine("sess02");
    let env = engine.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(!env.ok);
    assert_eq!(env.code, ErrorCode::SessionLost);
    assert_ne!(env.code, ErrorCode::Blocked);
    assert_ne!(env.code, ErrorCode::Parse);
    assert_ne!(env.code, ErrorCode::Unsupported);
    assert!(!env.error.is_empty());
    assert!(env.data.is_none());
    assert_eq!(env.validate(), Ok(()));
}

/// SESS-03：cookie 持久化——新引擎从磁盘恢复后无需重新登录。
#[tokio::test]
async fn sess_03_cookie_persists_across_engines() {
    let site = Site::start().await;
    let store = temp_store("sess03");
    let first = SessionEngine::with_store("default", store.clone()).expect("first");
    assert!(
        first
            .login(&site.url("/login"), None, &credentials())
            .await
            .ok
    );
    first.save_state(&[]).expect("save state");

    let mut second = SessionEngine::restore("default", store).expect("second");
    let env = second.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(env.ok, "{}", env.error);
    assert!(env.data.expect("data").html.contains("Welcome alice"));
    second.load_state().expect("load");
    assert!(!second.cookie_jar().is_empty());
}

/// SESS-04：CSRF token 从页面提取并回填，服务端断言匹配。
#[tokio::test]
async fn sess_04_csrf_is_extracted_and_refilled() {
    let site = Site::start().await;
    let engine = engine("sess04");
    let login = engine
        .login(&site.url("/login"), None, &credentials())
        .await;
    assert!(login.ok, "{}", login.error);

    let page = engine.fetch_page(&site.url("/form/basic")).await;
    let page = page.data.expect("page");
    let expected = page.forms[0]
        .field("csrf_token")
        .expect("token")
        .value
        .clone();
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("csrf_token"), Some(&expected));
}

/// SESS-05：token 轮换后仍能提交（使用新页面的新 token）。
#[tokio::test]
async fn sess_05_csrf_rotation_is_refetched() {
    let site = Site::start().await;
    let engine = engine("sess05");
    let page1 = engine
        .fetch_page(&site.url("/step1"))
        .await
        .data
        .expect("page1");
    let token1 = page1.forms[0]
        .field("csrf_token")
        .expect("t1")
        .value
        .clone();
    let step1 = engine.submit_form(&page1, 0, &FormFill::new()).await;
    assert!(step1.ok, "{}", step1.error);
    let page2 = step1.data.expect("page2");
    let token2 = page2.forms[0]
        .field("csrf_token")
        .expect("t2")
        .value
        .clone();
    assert_ne!(token1, token2, "服务端必须轮换 token");
    assert!(page2.html.contains("step1-done"));

    let step2 = engine.submit_form(&page2, 0, &FormFill::new()).await;
    assert!(step2.ok, "轮换后重取失败：{}", step2.error);
    assert!(step2.data.expect("page3").html.contains("all-done"));
}

/// SESS-06：隐藏字段在提交时保留原值。
#[tokio::test]
async fn sess_06_hidden_fields_are_preserved() {
    let site = Site::start().await;
    let engine = engine("sess06");
    let page = engine
        .fetch_page(&site.url("/form/basic"))
        .await
        .data
        .expect("page");
    let env = engine.submit_form(&page, 0, &FormFill::new()).await;
    assert!(env.ok, "{}", env.error);
    let recorded = site.with_state(|state| state.recorded("/form/landing"));
    assert_eq!(recorded.get("lang").map(String::as_str), Some("zh-CN"));
    assert_eq!(
        recorded.get("csrf_token").map(String::as_str),
        Some(
            page.forms[0]
                .field("csrf_token")
                .expect("token")
                .value
                .as_str()
        )
    );
}

/// SESS-07 + FORM-10：multipart 上传被服务端正确接收，字节数一致。
#[tokio::test]
async fn sess_07_form_10_multipart_upload() {
    let site = Site::start().await;
    let engine = engine("sess07");
    let page = engine
        .fetch_page(&site.url("/form/upload"))
        .await
        .data
        .expect("page");
    assert_eq!(
        page.forms[0].enctype,
        webctl_web::session::FormEnctype::Multipart
    );
    let mut fill = FormFill::new();
    fill.set("title", "报告").upload(
        "doc",
        FileUpload::new(
            "report.bin",
            "application/octet-stream",
            b"hello upload".to_vec(),
        ),
    );
    let env = engine.submit_form(&page, 0, &fill).await;
    assert!(env.ok, "{}", env.error);
    let (filename, bytes, title) = site
        .with_state(|state| state.upload.clone())
        .expect("upload");
    assert_eq!(filename, "report.bin");
    assert_eq!(bytes.as_slice(), b"hello upload");
    assert_eq!(title.as_deref(), Some("报告"));
}

/// SESS-08：分页遍历，各页条数之和 = 站点总数，且遵守页数上限。
#[tokio::test]
async fn sess_08_pagination_sums_items() {
    let site = Site::start().await;
    let engine = engine("sess08");
    let env = engine.paginate(&site.url("/list?page=1"), Some(10)).await;
    assert!(env.ok, "{}", env.error);
    assert_eq!(env.code, ErrorCode::Ok);
    let paginated = env.data.expect("paginated");
    assert_eq!(paginated.pages.len(), 3);
    assert_eq!(paginated.item_counts, vec![10, 10, 5]);
    assert_eq!(paginated.total_items, 25);
    assert!(paginated.limit.is_none());
    assert!(paginated.next.is_none());

    // 页数上限：只取一页即停止并标 LIMIT。
    let limited = engine.paginate(&site.url("/list?page=1"), Some(1)).await;
    assert_eq!(limited.code, ErrorCode::Limit);
    assert_eq!(
        limited.meta.degraded_from.as_deref(),
        Some("pagination-max-pages")
    );
    assert_eq!(limited.data.expect("data").pages.len(), 1);
}

/// SESS-09：循环 next（A→B→A）在上限内停止并标记 LIMIT。
#[tokio::test]
async fn sess_09_loop_pagination_stops_with_limit() {
    let site = Site::start().await;
    let engine = engine("sess09");
    let env = engine.paginate(&site.url("/loop?page=a"), Some(10)).await;
    assert_eq!(env.code, ErrorCode::Limit);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("pagination-loop"));
    assert_eq!(env.validate(), Ok(()));
    let paginated = env.data.expect("data");
    assert!(paginated.pages.len() <= 10, "必须在上限内停止");
    assert_eq!(paginated.pages.len(), 2);
    assert_eq!(paginated.limit.as_deref(), Some("pagination-loop"));
}

/// SESS-10：SPA 页面返回 DEGRADED 且 degraded_from 非空，不伪造成功。
#[tokio::test]
async fn sess_10_spa_degrades() {
    let site = Site::start().await;
    let engine = engine("sess10");
    let env = engine.fetch_page(&site.url("/spa")).await;
    assert_eq!(env.code, ErrorCode::Degraded);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("js-render"));
    assert_eq!(env.validate(), Ok(()));
    let page = env.data.expect("page");
    assert!(page.spa.is_some());
    assert!(page.spa.as_ref().expect("signal").reason.contains("JS"));
    assert!(page.html.contains("id=\"root\""));
}

/// SESS-11：登录态导出 → 新会话导入 → 受保护页可访问。
#[tokio::test]
async fn sess_11_export_import_session_state() {
    let site = Site::start().await;
    let store = temp_store("sess11");
    let first = SessionEngine::with_store("default", store.clone()).expect("first");
    assert!(
        first
            .login(&site.url("/login"), None, &credentials())
            .await
            .ok
    );
    let state = first.export_state(&[site.url("/").trim_end_matches('/').to_owned()]);
    let json = state.export_json().expect("export");
    let imported = webctl_web::session::SessionState::import_json(&json).expect("import");
    assert_eq!(imported.cookies.len(), state.cookies.len());

    let mut second = SessionEngine::with_store("default", store).expect("second");
    second.import_state(&imported);
    let env = second.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(env.ok, "{}", env.error);
    assert!(env.data.expect("data").html.contains("Welcome alice"));
}

/// SESS-12：非法会话名（路径穿越等）被拒，不落到 state 目录外。
#[tokio::test]
async fn sess_12_invalid_session_name_is_rejected() {
    let store = temp_store("sess12");
    let root = store.sessions_dir();
    for bad in ["../escape", "a/b", "a\\b", "", ".."] {
        assert!(validate_session_name(bad).is_err(), "{bad:?}");
        assert!(store.path_for(bad).is_err(), "{bad:?}");
        let err = SessionEngine::with_store(bad, store.clone()).expect_err("reject");
        assert_eq!(err.code(), ErrorCode::Blocked);
    }
    let outside = root.parent().expect("parent").join("escape.json");
    assert!(!outside.exists(), "不得在 state 目录外创建文件");
    assert!(!root.join("..").join("escape.json").exists());
}

/// SESS-13：登录后 302 回跳原页面，会话保持。
#[tokio::test]
async fn sess_13_redirect_login_keeps_session() {
    let site = Site::start().await;
    let engine = engine("sess13");
    // 未登录访问 /dashboard 会被 302 到 /login。
    let before = engine.fetch_page(&site.url("/dashboard")).await;
    assert!(before.ok, "{}", before.error);
    let page = before.data.expect("page");
    assert_eq!(page.url, site.url("/login"), "应跟随 302 到登录页");

    let env = engine
        .login(&site.url("/login"), None, &credentials())
        .await;
    assert!(env.ok, "{}", env.error);
    let landed = env.data.expect("landed");
    assert_eq!(landed.url, site.url("/dashboard"), "登录后应 302 回跳");
    let after = engine.fetch_authenticated(&site.url("/dashboard")).await;
    assert!(after.ok, "{}", after.error);
}

/// SESS-14：同一会话并发使用时结果正确（无串号）。
#[tokio::test]
async fn sess_14_concurrent_requests_share_session() {
    let site = Site::start().await;
    let engine = engine("sess14");
    assert!(
        engine
            .login(&site.url("/login"), None, &credentials())
            .await
            .ok
    );
    let dashboard = site.url("/dashboard");
    let (a, b) = tokio::join!(engine.fetch_page(&dashboard), engine.fetch_page(&dashboard));
    for env in [a, b] {
        assert!(env.ok, "{}", env.error);
        assert!(env.data.expect("data").html.contains("Welcome alice"));
    }
    assert!(site.hits("/dashboard") >= 2);
}

/// SESS-15：paginate 必须传播页面级降级——SPA 空壳只能是 DEGRADED，绝不 OK+0 条。
#[tokio::test]
async fn sess_15_paginate_spa_stays_degraded() {
    let site = Site::start().await;
    let engine = engine("sess15");
    let env = engine.paginate(&site.url("/spa"), Some(3)).await;
    assert_eq!(env.code, ErrorCode::Degraded, "SPA 分页不得被吞成 OK");
    assert_ne!(env.code, ErrorCode::Ok);
    assert_eq!(env.meta.degraded_from.as_deref(), Some("js-render"));
    assert_eq!(env.validate(), Ok(()));
    let paginated = env.data.expect("paginated");
    assert_eq!(paginated.pages.len(), 1);
    assert_eq!(paginated.total_items, 0);
    assert!(paginated.pages[0].spa.is_some());
}
