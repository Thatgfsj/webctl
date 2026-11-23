//! 地址守卫（#191）：传输层每次连接前拒绝非公网目标。
//!
//! 威胁模型：MCP 服务端被 agent 调用，攻击者可以用 URL 诱导 webctl 访问
//! 内网服务、本机回环服务或云 metadata 端点（SSRF）。"--offline" 只回答
//! “能不能联网”，不回答“能连到哪”，因此这里在**传输层**加一道地址检查。
//!
//! 规则（默认，见 AddressGuard）：
//! - 拒绝 loopback / RFC1918 私网 / 链路本地 / 组播 / 未指定 / CGNAT / 保留地址；
//! - 云 metadata（169.254.169.254 / [fd00:ec2::254]）**始终**拒绝，
//!   即使显式打开了放行开关（避免“本地联调顺手打开”变成凭据泄漏）；
//! - 主机名先解析、再对**全部**解析结果判定（任一被禁即拒绝），
//!   判定发生在发起连接之前——被拒时不会向目标发出任何请求。
//!
//! 逐跳：transport::run 与 SessionEngine::send 都是手工跟随重定向的循环，
//! 每一跳在发请求之前都会重新调用 AddressGuard::check_url，因此重定向到内网
//! 同样会被拒绝（不能只查入口 URL）。
//!
//! 诚实边界（写入 docs/CONFIGURATION.md）：
//! - **不防 DNS rebinding**：预检用系统解析器，真实连接由 wreq 再解析一次，
//!   两次结果理论上可以不同；本模块只保证“预检看到的地址”是公网的。
//! - **经代理时**只校验目标主机名，不校验代理自身的地址（代理由显式配置指定）。
//! - 非 http(s) scheme 在 parse_target 处已被拒绝，不进入本模块。

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, OnceLock};

use url::{Host, Url};
use webctl_core::ErrorCode;

/// 云 metadata 的 IPv4 端点（AWS / GCP / Azure / OpenStack 通用）。
pub const CLOUD_METADATA_IPV4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
/// 云 metadata 的 IPv6 端点（AWS IMDS 的 IPv6 形式）。
pub const CLOUD_METADATA_IPV6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

/// 地址分类（只用于解释拒绝原因与审计）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    /// 公网可路由地址。
    Public,
    /// 回环（127.0.0.0/8、::1）。
    Loopback,
    /// 私网（RFC1918 与 IPv6 ULA fc00::/7）。
    Private,
    /// 链路本地（169.254.0.0/16、fe80::/10）。
    LinkLocal,
    /// 组播（224.0.0.0/4、ff00::/8）。
    Multicast,
    /// 未指定 / “本网络”（0.0.0.0/8、::）。
    Unspecified,
    /// 运营商级 NAT（100.64.0.0/10）。
    Shared,
    /// 保留地址（240.0.0.0/4、广播地址）。
    Reserved,
    /// 云 metadata 端点。
    Metadata,
}

impl AddressClass {
    /// 稳定字符串（出现在错误信息与审计日志里）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::LinkLocal => "link-local",
            Self::Multicast => "multicast",
            Self::Unspecified => "unspecified",
            Self::Shared => "shared",
            Self::Reserved => "reserved",
            Self::Metadata => "cloud-metadata",
        }
    }

    /// 机器可读的拒绝原因码；Public 返回 None。
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Public => None,
            Self::Loopback => Some("loopback-address-blocked"),
            Self::Private => Some("private-address-blocked"),
            Self::LinkLocal => Some("link-local-address-blocked"),
            Self::Multicast => Some("multicast-address-blocked"),
            Self::Unspecified => Some("unspecified-address-blocked"),
            Self::Shared => Some("shared-address-blocked"),
            Self::Reserved => Some("reserved-address-blocked"),
            Self::Metadata => Some("metadata-endpoint-blocked"),
        }
    }

    /// 是否公网地址。
    pub fn is_public(self) -> bool {
        self == Self::Public
    }
}

/// 判定一个 IP 的分类（纯函数）。
pub fn classify(ip: IpAddr) -> AddressClass {
    match ip {
        IpAddr::V4(ip) => classify_v4(ip),
        IpAddr::V6(ip) => classify_v6(ip),
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddressClass {
    let octets = ip.octets();
    if ip == CLOUD_METADATA_IPV4 {
        AddressClass::Metadata
    } else if ip.is_loopback() {
        AddressClass::Loopback
    } else if ip.is_private() {
        AddressClass::Private
    } else if ip.is_link_local() {
        AddressClass::LinkLocal
    } else if ip.is_multicast() {
        AddressClass::Multicast
    } else if ip.is_unspecified() || octets[0] == 0 {
        AddressClass::Unspecified
    } else if octets[0] >= 240 {
        // 240.0.0.0/4（含 255.255.255.255 广播）不可作为目标。
        AddressClass::Reserved
    } else if octets[0] == 100 && (octets[1] & 0xc0) == 64 {
        AddressClass::Shared
    } else {
        AddressClass::Public
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddressClass {
    // ::ffff:a.b.c.d 按内层 IPv4 判定，避免用映射写法绕过守卫。
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return classify_v4(mapped);
    }
    if ip == CLOUD_METADATA_IPV6 {
        AddressClass::Metadata
    } else if ip.is_loopback() {
        AddressClass::Loopback
    } else if ip.is_unspecified() {
        AddressClass::Unspecified
    } else if ip.is_multicast() {
        AddressClass::Multicast
    } else if (ip.segments()[0] & 0xffc0) == 0xfe80 {
        AddressClass::LinkLocal
    } else if (ip.segments()[0] & 0xfe00) == 0xfc00 {
        AddressClass::Private
    } else {
        AddressClass::Public
    }
}

/// 被守卫拒绝的目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedAddress {
    /// 被拒绝的主机（域名或 IP 字面量）。
    pub host: String,
    /// 触发拒绝的解析地址。
    pub ip: IpAddr,
    /// 地址分类。
    pub class: AddressClass,
    /// 触发拒绝的完整 URL（纯函数判定时为 None）。
    pub url: Option<String>,
}

impl BlockedAddress {
    /// 机器可读原因码（例如 private-address-blocked）。
    pub fn reason(&self) -> &'static str {
        self.class.reason().unwrap_or("address-blocked")
    }

    /// 可行动的错误说明：原因码 + 目标 + 显式开关的用法。
    ///
    /// 这里刻意不复用网络错误的措辞：调用方需要能区分“目标不可达”和
    /// “webctl 主动拒绝了该地址”。
    pub fn message(&self) -> String {
        let target = self.url.as_deref().unwrap_or(self.host.as_str());
        if self.class == AddressClass::Metadata {
            return format!(
                "{}: 拒绝访问 {target}（解析到 {}，云 metadata 端点）。该端点始终被拒绝——即使开启 WEBCTL_ALLOW_PRIVATE_NETWORK 也不会放行，以免实例凭据被读取。",
                self.reason(),
                self.ip
            );
        }
        format!(
            "{}: 拒绝访问 {target}（解析到 {}，分类 {}）。webctl 默认拒绝 loopback / 私网 / 链路本地 / 组播 / 未指定 / 保留地址与云 metadata；确需访问可信的本地服务时，显式设置 WEBCTL_ALLOW_PRIVATE_NETWORK=1（或配置文件 allow_private_network = true），并只在受信任的环境中使用。",
            self.reason(),
            self.ip,
            self.class.as_str()
        )
    }
}

/// 守卫错误：地址被拒（BLOCKED）或解析失败（NETWORK）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    /// 命中地址守卫。
    Blocked(BlockedAddress),
    /// 主机名解析失败（不是守卫拒绝，不得伪装成 BLOCKED）。
    Resolve {
        /// 主机名。
        host: String,
        /// 底层错误说明。
        detail: String,
    },
}

impl GuardError {
    /// 归一后的错误码。
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Blocked(_) => ErrorCode::Blocked,
            Self::Resolve { .. } => ErrorCode::Network,
        }
    }

    /// 人类可读说明。
    pub fn message(&self) -> String {
        match self {
            Self::Blocked(blocked) => blocked.message(),
            Self::Resolve { host, detail } => {
                format!("无法解析目标主机 {host:?} 的地址：{detail}")
            }
        }
    }
}

/// 地址守卫：按显式放行开关决定是否允许非公网地址。
///
/// allow_private_network = false（默认）拒绝全部非公网分类；
/// true 放行除云 metadata 外的非公网分类（仅供可信本地联调）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressGuard {
    allow_private_network: bool,
}

impl Default for AddressGuard {
    fn default() -> Self {
        Self::new(false)
    }
}

impl AddressGuard {
    /// 构造守卫。
    pub const fn new(allow_private_network: bool) -> Self {
        Self {
            allow_private_network,
        }
    }

    /// 是否显式放行了非公网地址。
    pub const fn allow_private_network(self) -> bool {
        self.allow_private_network
    }

    /// 对“主机名 + 已解析地址集合”判定（纯函数，零 DNS、零连接）。
    ///
    /// 这是守卫的判定核心：集成测试用合成地址集合模拟“重定向第二跳”，
    /// 不需要真实 DNS 或外网。任一地址被禁即拒绝（不放过混合应答）。
    pub fn check_resolved(&self, host: &str, addrs: &[IpAddr]) -> Result<(), BlockedAddress> {
        for ip in addrs {
            let class = classify(*ip);
            if class == AddressClass::Metadata {
                return Err(BlockedAddress {
                    host: host.to_owned(),
                    ip: *ip,
                    class,
                    url: None,
                });
            }
            if class.is_public() {
                continue;
            }
            if self.allow_private_network {
                audit_allow_once(host, *ip);
                continue;
            }
            return Err(BlockedAddress {
                host: host.to_owned(),
                ip: *ip,
                class,
                url: None,
            });
        }
        Ok(())
    }

    /// 解析并校验 URL 的主机；判定发生在**发起连接之前**。
    pub async fn check_url(&self, url: &Url) -> Result<(), GuardError> {
        let Some(host) = url.host() else {
            return Err(GuardError::Resolve {
                host: url.as_str().to_owned(),
                detail: "URL 缺少主机".to_owned(),
            });
        };
        match host {
            Host::Ipv4(ip) => self.check_resolved(&ip.to_string(), &[IpAddr::V4(ip)]),
            Host::Ipv6(ip) => self.check_resolved(&ip.to_string(), &[IpAddr::V6(ip)]),
            Host::Domain(domain) => {
                let port = url.port_or_known_default().unwrap_or(80);
                let resolved = tokio::net::lookup_host((domain, port))
                    .await
                    .map_err(|error| GuardError::Resolve {
                        host: domain.to_owned(),
                        detail: error.to_string(),
                    })?;
                let addrs: Vec<IpAddr> = resolved.map(|addr| addr.ip()).collect();
                if addrs.is_empty() {
                    return Err(GuardError::Resolve {
                        host: domain.to_owned(),
                        detail: "解析结果为空".to_owned(),
                    });
                }
                self.check_resolved(domain, &addrs)
            }
        }
        .map_err(GuardError::Blocked)
    }
}

/// 审计日志：显式放行开关生效时，按 (主机, IP) 去重后写一行到 stderr。
///
/// 去重是必要的：夹具测试会反复访问同一个回环地址，逐次打印会淹没输出。
fn audit_allow_once(host: &str, ip: IpAddr) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut seen) = seen.lock() else {
        return;
    };
    if seen.insert(format!("{host}|{ip}")) {
        eprintln!(
            "webctl-audit: WEBCTL_ALLOW_PRIVATE_NETWORK 放行了非公网目标 {host} -> {ip}（仅限可信本地联调；生产环境请关闭该开关）"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("ip literal")
    }

    /// 分类表：默认拒绝面覆盖 issue #191 点名的全部分类。
    #[test]
    fn classifies_non_public_addresses() {
        for (text, expected) in [
            ("127.0.0.1", AddressClass::Loopback),
            ("127.9.9.9", AddressClass::Loopback),
            ("::1", AddressClass::Loopback),
            ("10.1.2.3", AddressClass::Private),
            ("172.16.0.1", AddressClass::Private),
            ("192.168.1.1", AddressClass::Private),
            ("fd12:3456::1", AddressClass::Private),
            ("169.254.1.1", AddressClass::LinkLocal),
            ("fe80::1", AddressClass::LinkLocal),
            ("224.0.0.1", AddressClass::Multicast),
            ("ff02::1", AddressClass::Multicast),
            ("0.0.0.0", AddressClass::Unspecified),
            ("::", AddressClass::Unspecified),
            ("100.64.0.1", AddressClass::Shared),
            ("240.0.0.1", AddressClass::Reserved),
            ("255.255.255.255", AddressClass::Reserved),
            ("169.254.169.254", AddressClass::Metadata),
            ("fd00:ec2::254", AddressClass::Metadata),
        ] {
            let got = classify(ip(text));
            assert_eq!(got, expected, "{text}");
            assert!(!got.is_public(), "{text} 不得被当作公网地址");
            assert!(got.reason().is_some(), "{text} 必须有原因码");
        }
    }

    /// 公网地址不误杀（对照组，证明守卫不是“一律拒绝”）。
    #[test]
    fn classifies_public_addresses() {
        for text in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(classify(ip(text)).is_public(), "{text} 应是公网地址");
        }
    }

    /// IPv4 映射到 IPv6 的写法不能绕过守卫。
    #[test]
    fn ipv4_mapped_ipv6_is_classified_by_inner_address() {
        assert_eq!(classify(ip("::ffff:127.0.0.1")), AddressClass::Loopback);
        assert_eq!(
            classify(ip("::ffff:169.254.169.254")),
            AddressClass::Metadata
        );
        assert_eq!(classify(ip("::ffff:10.0.0.1")), AddressClass::Private);
    }

    /// 默认拒绝：每个分类都被拒，且错误码是 BLOCKED 而不是 NETWORK。
    #[test]
    fn default_guard_rejects_every_non_public_class() {
        let guard = AddressGuard::default();
        for text in [
            "127.0.0.1",
            "::1",
            "10.0.0.5",
            "192.168.0.10",
            "169.254.169.254",
            "224.0.0.1",
            "0.0.0.0",
            "100.64.1.1",
            "240.0.0.1",
        ] {
            let error = guard
                .check_resolved("synthetic.test", &[ip(text)])
                .expect_err(text);
            assert_eq!(error.ip, ip(text));
            assert!(error.reason().ends_with("-blocked"), "{text}");
        }
        let blocked = guard
            .check_resolved("synthetic.test", &[ip("127.0.0.1")])
            .expect_err("loopback");
        let error = GuardError::Blocked(blocked);
        assert_eq!(error.code(), ErrorCode::Blocked);
        assert!(
            error.message().contains("WEBCTL_ALLOW_PRIVATE_NETWORK"),
            "错误说明必须可行动：{}",
            error.message()
        );
        assert!(
            error.message().contains("loopback-address-blocked"),
            "错误说明必须带原因码：{}",
            error.message()
        );
    }

    /// 混合应答：只要有一个被禁地址就拒绝（不放过 DNS 多答案绕过）。
    #[test]
    fn mixed_answers_are_rejected_when_any_address_is_private() {
        let guard = AddressGuard::default();
        let error = guard
            .check_resolved("mixed.test", &[ip("93.184.216.34"), ip("10.0.0.5")])
            .expect_err("私网混入公网应答");
        assert_eq!(error.class, AddressClass::Private);
    }

    /// 显式放行：回环/私网/链路本地可用，但 metadata 仍被拒绝。
    #[test]
    fn allow_switch_permits_private_but_never_metadata() {
        let guard = AddressGuard::new(true);
        for text in ["127.0.0.1", "10.0.0.5", "fe80::1", "100.64.0.1"] {
            assert!(
                guard.check_resolved("local.test", &[ip(text)]).is_ok(),
                "{text} 在放行开关下应可用"
            );
        }
        let error = guard
            .check_resolved("metadata.test", &[ip("169.254.169.254")])
            .expect_err("metadata 必须始终拒绝");
        assert_eq!(error.class, AddressClass::Metadata);
        assert_eq!(error.reason(), "metadata-endpoint-blocked");
    }

    /// 合成重定向链：第一跳公网通过，第二跳私网被拒（逐跳判定的核心断言）。
    #[test]
    fn synthetic_redirect_chain_is_checked_per_hop() {
        let guard = AddressGuard::default();
        let chain: [(&str, [&str; 1]); 2] = [
            ("public.example", ["93.184.216.34"]),
            ("internal.example", ["10.0.0.5"]),
        ];
        let mut visited = Vec::new();
        for (host, addrs) in chain {
            let parsed: Vec<IpAddr> = addrs.iter().map(|text| ip(text)).collect();
            match guard.check_resolved(host, &parsed) {
                Ok(()) => visited.push(host),
                Err(error) => {
                    assert_eq!(visited, vec!["public.example"], "只应通过第一跳");
                    assert_eq!(error.reason(), "private-address-blocked");
                    assert_eq!(error.host, "internal.example");
                    return;
                }
            }
        }
        panic!("第二跳必须被拒绝");
    }

    /// 错误说明不能伪装成网络错误：不得出现 connection failed / 超时措辞。
    #[test]
    fn blocked_message_is_not_network_language() {
        let blocked = AddressGuard::default()
            .check_resolved("127.0.0.1", &[ip("127.0.0.1")])
            .expect_err("loopback");
        let message = GuardError::Blocked(blocked).message();
        let lowered = message.to_lowercase();
        assert!(!lowered.contains("connection failed"), "{message}");
        assert!(!lowered.contains("timed out"), "{message}");
        assert!(message.contains("拒绝"), "{message}");
    }
}
