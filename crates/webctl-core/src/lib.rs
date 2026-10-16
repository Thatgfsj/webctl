//! webctl 核心契约层。
//!
//! 提供结果信封、错误码封闭集、运行配置与内置能力自检（caps）。
//! 依赖方向：被所有上层依赖，自身不依赖任何上层（PLAN v0.4 §4.2）。
//!
//! 本 crate 不含网络、解析或子进程逻辑：运行时零外部进程是硬约束（PLAN v0.4 §3.1）。

pub mod caps;
pub mod config;
pub mod envelope;
pub mod error;
pub mod state;

pub use caps::{Capability, CapabilityDomain, CapabilityState, CapsReport};
pub use config::{
    Config, ConfigError, ConfigLayer, ENV_VARS, OutputFormat, default_config_path,
    load_config_file, redact_url_userinfo, resolve,
};
pub use envelope::{Engine, Envelope, EnvelopeViolation, Meta, Trust};
pub use error::ErrorCode;
