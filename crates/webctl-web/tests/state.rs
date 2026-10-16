//! STATE-01…09 的离线用例（PLAN v0.4 §3.3 / docs/TESTING.md §2.6，PR 05 / #34）。
//!
//! 全部用例只使用临时目录与 127.0.0.1，不访问外网、不创建子进程（零子进程硬约束）。
//! “两个进程同时写”用两个独立打开的文件句柄（同一进程内两个线程 / 两把句柄）
//! 验证：flock 的锁属于 open file description，两个独立 open 会真实冲突，
//! 足以验证“后到者等待或明确报错”。真正的多进程场景由同一把锁的语义保证。

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use webctl_core::ErrorCode;
use webctl_core::state::{self, FileLock, dir_mode, file_mode};
use webctl_web::session::{SessionError, SessionStore, validate_session_name};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 唯一的临时 state dir；Drop 时递归清理。
struct TempState {
    path: PathBuf,
}

impl TempState {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let seq = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("webctl-pr05-{label}-{nanos}-{seq}"));
        fs::create_dir_all(&path).expect("create temp state dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn store(&self) -> SessionStore {
        SessionStore::with_ttl(&self.path, Duration::from_secs(3600))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn temp_state(label: &str) -> TempState {
    TempState::new(label)
}

fn leftovers(dir: &Path, needle: &str) -> Vec<String> {
    fs::read_dir(dir)
        .expect("read sessions dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(needle))
        .collect()
}

/// STATE-01：state dir 700、会话/凭据文件 600（非 Unix 显式 None）。
#[test]
fn state_01_directory_0700_and_files_0600() {
    let tmp = temp_state("state01");
    let store = tmp.store();
    let mut session = store.new_state("default").expect("state");
    session.note_origin("http://127.0.0.1:1");
    store.save(&session).expect("save");

    let sessions = store.sessions_dir();
    let session_file = store.path_for("default").expect("session path");
    let lock_file = store.lock_path("default").expect("lock path");

    #[cfg(unix)]
    {
        assert_eq!(dir_mode(tmp.path()), Some(0o700), "state dir 必须 0700");
        assert_eq!(dir_mode(&sessions), Some(0o700), "sessions dir 必须 0700");
        assert_eq!(file_mode(&session_file), Some(0o600), "会话文件必须 0600");
        assert_eq!(file_mode(&lock_file), Some(0o600), "锁文件必须 0600");
    }
    #[cfg(not(unix))]
    {
        assert_eq!(
            dir_mode(tmp.path()),
            None,
            "非 Unix 必须显式 None，而不是假通过"
        );
        assert_eq!(dir_mode(&sessions), None);
        assert_eq!(file_mode(&session_file), None);
        assert_eq!(file_mode(&lock_file), None);
    }
}

/// STATE-02：两个独立写者并发写同一会话不损坏；后到者明确等待，绝不静默丢弃。
#[test]
/// 不变量：**并发写不得损坏**（最终文件完整可解析、只含一次完整写入、无 .tmp 残留）。
///
/// **不**断言「每一次写都成功」：锁等待是有界的，争抢激烈时 `LockTimeout` 是**设计内结果**
/// （STATE-03 单独锁定该行为）。原写法把「不损坏」与「每次都赢」混在一起断言，
/// 在慢 runner 上随机假红（见议题 #85）。这里只容忍 `LockTimeout`，其它错误一律 panic。
fn state_02_concurrent_writers_do_not_corrupt() {
    let tmp = temp_state("state02");
    let store = tmp.store();
    store
        .save(&store.new_state("contested").expect("state"))
        .expect("seed");

    let writers = 8;
    let rounds = 12;
    let mut handles = Vec::new();
    for index in 0..writers {
        let store = store.clone();
        handles.push(thread::spawn(move || {
            let mut saved = 0usize;
            let mut lock_timeouts = 0usize;
            for round in 0..rounds {
                let mut session = store.new_state("contested").expect("state");
                session.note_origin(&format!("http://127.0.0.1/{index}/{round}"));
                match store.save(&session) {
                    Ok(_) => saved += 1,
                    // 有界锁等待超时是**设计内的结果**（争抢激烈时等待者放弃，见 STATE-03），
                    // 不是损坏 —— 本用例要证明的是「不得损坏」，不是「每次都赢」。
                    Err(SessionError::LockTimeout(_)) => lock_timeouts += 1,
                    // 其它任何错误都是真问题，绝不放过。
                    Err(other) => panic!("并发写出现非锁错误：{other:?}"),
                }
            }
            (saved, lock_timeouts)
        }));
    }
    let (mut saved, mut lock_timeouts) = (0usize, 0usize);
    for handle in handles {
        let (saved_here, timeouts_here) = handle.join().expect("writer thread");
        saved += saved_here;
        lock_timeouts += timeouts_here;
    }
    eprintln!("并发写结果：saved={saved} lock_timeouts={lock_timeouts}");
    assert!(
        saved > 0,
        "至少要成功写入一次，否则本用例没有验证到任何东西"
    );

    let loaded = store.load("contested").expect("最终文件必须可解析");
    assert_eq!(loaded.name, "contested");
    assert_eq!(
        loaded.origins.len(),
        1,
        "只应是某一次完整写入，而不是合并/半截"
    );
    assert!(
        leftovers(&store.sessions_dir(), ".tmp").is_empty(),
        "临时文件不得残留"
    );
}

/// STATE-03：持锁进程被杀后无死锁；锁被占用时给出可读超时；释放后等待者成功。
#[test]
fn state_03_bounded_lock_wait_and_no_deadlock() {
    let tmp = temp_state("state03");
    let store = tmp.store();

    // 1) 残留锁文件（模拟持锁进程被杀）不造成死锁。
    let lock_path = store.lock_path("stale").expect("lock path");
    fs::create_dir_all(store.sessions_dir()).expect("sessions dir");
    fs::write(&lock_path, b"stale pid 99999").expect("stale lock file");
    let quick = store.clone().with_lock_wait(Duration::from_millis(300));
    let session = quick.new_state("stale").expect("state");
    quick.save(&session).expect("残留锁文件不得阻塞新写者");

    // 2) 真正持锁：后到者得到可读错误，而不是静默丢弃。
    let held = FileLock::acquire(&lock_path, Duration::from_millis(200)).expect("hold lock");
    let blocked = store.clone().with_lock_wait(Duration::from_millis(80));
    let error = blocked.save(&session).expect_err("必须返回锁超时");
    assert!(matches!(error, SessionError::LockTimeout(_)), "{error:?}");
    assert_eq!(error.code(), ErrorCode::SessionLost);
    assert!(error.message().contains("timed out"), "{}", error.message());

    // 3) 持锁者释放后，写者明确等待并成功。
    held.release();
    store
        .clone()
        .with_lock_wait(Duration::from_secs(2))
        .save(&session)
        .expect("释放后必须成功");

    // 4) 后到者确实等待：独立句柄持锁一段时间，写者等到后成功。
    // 用**信道同步**代替 `sleep(40ms)` 来假定「持锁者已经开始」：
    // `thread::sleep` 只保证「至少睡这么久」，负载高时会**超睡**，把计时起点往后推、
    // 从而把可测量的等待压缩到阈值以下（macOS CI 实测 44.9ms < 50ms 即此因，见议题 #85）。
    // 先等到「已持锁」信号再起表，就把这段不确定性消掉；持有时长也放大到 8× 余量。
    let waited_lock = store.lock_path("waited").expect("lock path");
    let holder_path = waited_lock.clone();
    let (locked_tx, locked_rx) = mpsc::channel::<()>();
    let holder = thread::spawn(move || {
        let lock = FileLock::acquire(&holder_path, Duration::from_secs(1)).expect("holder");
        locked_tx.send(()).expect("signal locked");
        thread::sleep(Duration::from_millis(400));
        lock.release();
    });
    locked_rx.recv().expect("holder 必须已持锁");
    let waited_state = store.new_state("waited").expect("state");
    let started = Instant::now();
    store
        .clone()
        .with_lock_wait(Duration::from_secs(3))
        .save(&waited_state)
        .expect("等待者必须成功，而不是被静默丢弃");
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(50),
        "应当真实等待过锁，实际 {waited:?}"
    );
    holder.join().expect("holder thread");
}

/// STATE-04：过期会话可回收；访问过期会话给出可读错误，不静默新建。
#[test]
fn state_04_ttl_reclaims_and_expired_access_is_readable() {
    let tmp = temp_state("state04");
    let store = tmp.store();

    let mut expired = store.new_state("old").expect("state");
    expired.expires_at = Some(1);
    store.save(&expired).expect("save expired");
    let path = store.path_for("old").expect("path");
    assert!(path.exists(), "过期文件应仍在磁盘上");

    let error = store.load("old").expect_err("过期必须报错");
    assert!(matches!(error, SessionError::Expired(_)), "{error:?}");
    assert_eq!(error.code(), ErrorCode::SessionLost);
    assert!(!error.message().is_empty());
    assert!(path.exists(), "访问过期会话不得静默删除或重建");

    let fresh = store.new_state("fresh").expect("state");
    store.save(&fresh).expect("save fresh");

    let purged = store.purge_expired().expect("purge");
    assert_eq!(purged, 1, "只回收 1 个过期会话");
    assert!(matches!(store.load("old"), Err(SessionError::NotFound(_))));
    assert!(!path.exists(), "回收后文件应被删除");
    assert!(store.load("fresh").is_ok(), "未过期会话必须保留");
}

/// STATE-05：截断/乱码的会话文件 → 明确错误码，不静默重建。
#[test]
fn state_05_corrupt_file_is_explicit_and_not_rebuilt() {
    let tmp = temp_state("state05");
    let store = tmp.store();
    store
        .save(&store.new_state("broken").expect("state"))
        .expect("save");
    let path = store.path_for("broken").expect("path");

    let truncated = b"{\"version\":1,\"name\":\"broken\"".to_vec();
    fs::write(&path, &truncated).expect("truncate");
    let error = store.load("broken").expect_err("截断文件必须报错");
    assert!(matches!(error, SessionError::Corrupt(_)), "{error:?}");
    assert_eq!(error.code(), ErrorCode::SessionLost);
    assert!(error.message().contains("corrupt"), "{}", error.message());
    assert_eq!(
        fs::read(&path).expect("read"),
        truncated,
        "损坏文件不得被静默重建或覆盖"
    );

    // 乱码（非 UTF-8）同样是损坏，而不是成功或半截。
    fs::write(&path, [0xff, 0xfe, 0x00, 0x01]).expect("binary garbage");
    let error = store.load("broken").expect_err("乱码必须报错");
    assert!(matches!(error, SessionError::Corrupt(_)), "{error:?}");
    assert_eq!(error.code(), ErrorCode::SessionLost);
}

/// STATE-06：CLI 与 MCP 共享同一 state dir（解析一致 + 落盘互见 + 同一把锁）。
#[test]
fn state_06_cli_and_mcp_share_one_state_dir() {
    // 优先级用注入值覆盖（edition 2024 下 set_var 是 unsafe，本 workspace 禁止）。
    let env_dir = Path::new("/tmp/webctl-shared-env-state");
    let home = OsStr::new("/home/u");
    let cli = state::resolve_state_dir_from(None, Some(env_dir.as_os_str()), Some(home));
    let mcp = state::resolve_state_dir_from(None, Some(env_dir.as_os_str()), Some(home));
    assert_eq!(cli, mcp, "两次独立解析必须得到同一路径");
    assert_eq!(cli, env_dir.to_path_buf());
    let explicit = Path::new("/tmp/webctl-shared-explicit-state");
    assert_eq!(
        state::resolve_state_dir_from(Some(explicit), Some(env_dir.as_os_str()), Some(home)),
        explicit.to_path_buf(),
        "显式参数（CLI 标志）必须压过环境变量"
    );
    assert_eq!(
        state::resolve_state_dir_from(None, None, Some(home)),
        PathBuf::from(home).join(state::STATE_DIR_NAME)
    );
    assert_eq!(
        state::resolve_state_dir(None),
        webctl_core::config::default_state_dir(),
        "单一入口与既有配置解析必须一致"
    );

    // 进程外可见性：CLI 写入的会话，MCP 用独立 store 读得到。
    let tmp = temp_state("state06");
    let cli_store = SessionStore::new(tmp.path());
    let mut session = cli_store.new_state("shared").expect("state");
    session.note_origin("http://127.0.0.1:1");
    cli_store.save(&session).expect("cli save");

    let mcp_store = SessionStore::new(tmp.path());
    let seen = mcp_store.load("shared").expect("mcp 必须看见 cli 的会话");
    assert_eq!(seen.origins, vec!["http://127.0.0.1:1".to_owned()]);

    // 同一把锁：一个前端持锁，另一个前端明确超时 ⇒ 看的是同一份状态。
    let lock_path = cli_store.lock_path("shared").expect("lock path");
    let held = FileLock::acquire(&lock_path, Duration::from_millis(200)).expect("hold");
    let blocked = mcp_store.clone().with_lock_wait(Duration::from_millis(60));
    let error = blocked.load("shared").expect_err("共享锁必须可见");
    assert!(matches!(error, SessionError::LockTimeout(_)), "{error:?}");
    held.release();
}

/// STATE-07：会话名不能逃出 state dir（词法 + 绝对路径）。
#[test]
fn state_07_path_traversal_is_rejected() {
    let tmp = temp_state("state07");
    let store = tmp.store();
    for bad in [
        "../escape",
        "a/b",
        "a\\b",
        "",
        "..",
        ".hidden",
        "C:/windows",
    ] {
        assert!(validate_session_name(bad).is_err(), "{bad:?}");
        assert!(store.path_for(bad).is_err(), "{bad:?}");
        let error = store.load(bad).expect_err("必须拒绝");
        assert_eq!(error.code(), ErrorCode::Blocked, "{bad:?} -> {error:?}");
    }
    // state 目录内外都不得出现逃逸文件。
    assert_eq!(store.sessions_dir().parent().expect("parent"), tmp.path());
    assert!(!tmp.path().join("escape.json").exists());
    assert!(!tmp.path().join("..").join("escape.json").exists());
}

/// STATE-07：越界 symlink（含坏链接）被拒绝，不写到目录外。
#[cfg(unix)]
#[test]
fn state_07_symlink_escape_is_rejected() {
    use std::os::unix::fs::symlink;
    let tmp = temp_state("state07-link");
    let outside = temp_state("state07-outside");
    let store = SessionStore::new(tmp.path());
    let sessions = store.sessions_dir();
    fs::create_dir_all(&sessions).expect("sessions dir");

    let victim = outside.path().join("victim.json");
    fs::write(&victim, b"{}").expect("victim");
    symlink(&victim, sessions.join("linked.json")).expect("symlink");
    let error = store.path_for("linked").expect_err("越界 symlink 必须被拒");
    assert_eq!(error.code(), ErrorCode::Blocked, "{error:?}");
    assert!(matches!(error, SessionError::UnsafePath(_)));

    let mut session = store.new_state("linked").expect("state");
    session.note_origin("http://127.0.0.1:1");
    let save_error = store.save(&session).expect_err("save 必须拒绝");
    assert_eq!(save_error.code(), ErrorCode::Blocked);
    assert_eq!(
        fs::read(&victim).expect("victim"),
        b"{}",
        "目录外文件不得被覆盖"
    );

    symlink(
        outside.path().join("missing.json"),
        sessions.join("broken.json"),
    )
    .expect("broken link");
    let error = store.path_for("broken").expect_err("坏链接必须被拒");
    assert_eq!(error.code(), ErrorCode::Blocked, "{error:?}");
}

/// STATE-08：写入中断后文件仍是旧值或新值，不出现半截。
#[test]
fn state_08_atomic_write_never_exposes_partial_file() {
    let tmp = temp_state("state08");
    let store = tmp.store();

    let mut old = store.new_state("atomic").expect("state");
    old.note_origin("http://127.0.0.1/old");
    store.save(&old).expect("save old");
    assert_eq!(
        store.load("atomic").expect("load old").origins,
        vec!["http://127.0.0.1/old".to_owned()]
    );

    // 模拟写入中断：留下半截临时文件，绝不能被当成会话读取。
    let interrupted = store.sessions_dir().join(".atomic.tmpinterrupted");
    fs::write(&interrupted, b"{\"version\":1,\"name\":\"atomic\"").expect("partial temp");
    assert_eq!(
        store
            .load("atomic")
            .expect("半截临时文件必须被忽略")
            .origins,
        vec!["http://127.0.0.1/old".to_owned()]
    );

    let mut new = store.new_state("atomic").expect("state");
    new.note_origin("http://127.0.0.1/new");
    store.save(&new).expect("save new");
    assert_eq!(
        store.load("atomic").expect("load new").origins,
        vec!["http://127.0.0.1/new".to_owned()]
    );
    assert!(
        leftovers(&store.sessions_dir(), ".tmp").is_empty(),
        "save 后不得残留临时文件"
    );

    // 并发写 + 读：每次成功读取都必须是完整状态。
    let writer_store = store.clone();
    let writer = thread::spawn(move || {
        for round in 0..30 {
            let mut session = writer_store.new_state("atomic").expect("state");
            session.note_origin(&format!("http://127.0.0.1/r{round}"));
            writer_store.save(&session).expect("save");
        }
    });
    let mut reads = 0;
    for _ in 0..300 {
        if let Ok(state) = store.load("atomic") {
            assert_eq!(state.origins.len(), 1, "读取到半截状态");
            reads += 1;
        }
        if reads >= 20 && writer.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    writer.join().expect("writer");
    assert!(reads >= 1, "至少成功读取一次");
}

/// STATE-09：清理只删自己的会话，不影响其他会话。
#[test]
fn state_09_cleanup_is_isolated_per_session() {
    let tmp = temp_state("state09");
    let store = tmp.store();
    for name in ["alpha", "beta", "gamma"] {
        let mut session = store.new_state(name).expect("state");
        session.note_origin(&format!("http://127.0.0.1/{name}"));
        store.save(&session).expect("save");
    }
    let mut beta = store.load("beta").expect("beta");
    beta.expires_at = Some(1);
    store.save(&beta).expect("expire beta");

    assert_eq!(
        store.list().expect("list"),
        vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()]
    );

    assert!(store.remove("alpha").expect("remove alpha"));
    assert!(matches!(
        store.load("alpha"),
        Err(SessionError::NotFound(_))
    ));
    assert!(store.load("gamma").is_ok(), "remove 不得影响其他会话");
    assert!(matches!(store.load("beta"), Err(SessionError::Expired(_))));

    let purged = store.purge_expired().expect("purge");
    assert_eq!(purged, 1, "只回收过期的 beta");
    assert!(matches!(store.load("beta"), Err(SessionError::NotFound(_))));
    assert!(store.load("gamma").is_ok(), "purge 不得影响未过期会话");
    assert_eq!(store.list().expect("list"), vec!["gamma".to_owned()]);
    assert!(
        !store.remove("alpha").expect("remove missing"),
        "删除不存在的会话返回 false"
    );
}

/// STATE-10（回归，阻塞项 1）：`<state>/sessions` 本身是指向 state dir 之外的
/// symlink 时，save / load / remove / list / purge_expired 必须在 **每个入口**
/// 就返回可读的 UnsafePath（映射 BLOCKED），不得读、写或删掉外部文件；任何
/// 返回路径的函数（path_for / lock_path）同样拒绝。
///
/// 构造：
/// - `<state>/sessions` -> `<outside>`（越界 symlink）；
/// - `<outside>/expired.json` 是一个**已过期**的会话文件（最危险场景：修复前
///   purge_expired 会真的把它删掉）；
/// - `<outside>/sentinel.json` 是内容固定的哨兵（证明未被删除/改写）。
#[cfg(unix)]
#[test]
fn state_10_sessions_symlink_escape_is_blocked_at_every_entry() {
    use std::os::unix::fs::symlink;

    let tmp = temp_state("state10");
    let outside = temp_state("state10-outside");

    // 外部目录里的哨兵与“已过期会话”。
    let sentinel = outside.path().join("sentinel.json");
    let sentinel_bytes = b"outside-sentinel-do-not-touch";
    fs::write(&sentinel, sentinel_bytes).expect("sentinel");

    let expired_path = outside.path().join("expired.json");
    let mut expired = SessionStore::new(tmp.path())
        .new_state("expired")
        .expect("state");
    expired.expires_at = Some(1);
    let expired_bytes = expired.export_json().expect("export");
    fs::write(&expired_path, &expired_bytes).expect("write expired");

    // <state>/sessions 指向 state dir 之外的目录。
    let sessions = tmp.path().join("sessions");
    symlink(outside.path(), &sessions).expect("sessions symlink");

    let store = tmp.store();
    let assert_blocked = |result: Result<(), SessionError>, what: &str| {
        let error = result.expect_err(what);
        assert!(
            matches!(error, SessionError::UnsafePath(_)),
            "{what}: 期望 UnsafePath，实际 {error:?}"
        );
        assert_eq!(error.code(), ErrorCode::Blocked, "{what}");
        assert!(!error.message().is_empty(), "{what}: 必须可读");
    };

    // 任何返回路径的函数。
    assert_blocked(store.path_for("expired").map(|_| ()), "path_for");
    assert_blocked(store.lock_path("expired").map(|_| ()), "lock_path");

    // 每个数据入口：一个都不得静默成功。
    let mut fresh = store.new_state("fresh").expect("state");
    fresh.note_origin("http://127.0.0.1/1");
    assert_blocked(store.save(&fresh).map(|_| ()), "save");
    assert_blocked(store.load("expired").map(|_| ()), "load");
    assert_blocked(store.remove("expired").map(|_| ()), "remove");
    assert_blocked(store.list().map(|_| ()), "list");
    assert_blocked(store.purge_expired().map(|_| ()), "purge_expired");

    // 越界哨兵证据：外部文件既未被删、也未被改。
    assert!(sentinel.exists(), "外部哨兵文件不得被删除");
    assert_eq!(
        fs::read(&sentinel).expect("sentinel read"),
        sentinel_bytes.as_slice(),
        "外部哨兵内容不得被改写"
    );
    assert!(expired_path.exists(), "外部过期会话文件不得被 purge 删除");
    assert_eq!(
        fs::read(&expired_path).expect("expired read"),
        expired_bytes.as_bytes(),
        "外部过期会话文件不得被改写"
    );
    // sessions 仍是越界 symlink，state dir 内没有留下任何文件。
    assert!(
        fs::symlink_metadata(&sessions)
            .expect("sessions metadata")
            .file_type()
            .is_symlink(),
        "sessions 应仍是指向外部目录的 symlink"
    );
    assert!(
        fs::read_dir(&sessions)
            .expect("read sessions symlink")
            .flatten()
            .any(|entry| entry.file_name() == "sentinel.json"),
        "哨兵应仍在外部目录中（证明未被搬移/删除）"
    );
}
