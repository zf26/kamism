//! 出站请求目标校验 —— 防 SSRF。
//!
//! ── 为什么需要这一层 ────────────────────────────────────────────────────
//!
//! Webhook 的目标地址是**商户自己填的**，而注册是开放的（邮箱验证码即可）。
//! 改造前只校验了协议前缀：
//!
//! ```rust,ignore
//! if !url.starts_with("http://") && !url.starts_with("https://") { ... }
//! ```
//!
//! 于是任何注册用户都能把回调地址填成：
//!
//! * `http://127.0.0.1:5432` —— 本机 PostgreSQL
//! * `http://redis:6379` / `http://app:9527/health` —— Docker 网络内的其它服务
//! * `http://169.254.169.254/latest/meta-data/` —— 云厂商元数据，**可直接拿到临时凭证**
//! * 内网任意网段 + 任意端口 —— 一台免费的内网扫描器
//!
//! 而且**不是盲打**：投递日志里会记 `status=...`，商户通过"有没有收到回调"就能
//! 判断端口是否开放（半开连接、400、404、超时的表现各不相同）。
//!
//! ── 三道闸 ──────────────────────────────────────────────────────────────
//!
//! 1. **解析出 IP 并校验**（本模块）：私有段 / 回环 / 链路本地 / CGNAT /
//!    组播 / 保留段全部拒绝，IPv6 与 `::ffff:` 映射一并处理。
//! 2. **禁止重定向**：reqwest 默认跟随重定向，一条 `302 → 127.0.0.1` 就能绕过
//!    域名黑名单。调用方必须设 `redirect::Policy::none()`。
//! 3. **把域名钉死到校验过的 IP**：校验时解析一次、reqwest 建连时再解析一次，
//!    中间这个窗口就是 DNS rebinding（第一次返回公网 IP 通过校验，第二次返回
//!    127.0.0.1 完成攻击）。用 [`OutboundTarget::pin`] 把结果钉进客户端即可闭合。
//!
//! 三道缺一不可：只做 1 会被 302 绕过；只做 1+2 会被 rebinding 绕过。
//!
//! ── 已知边界 ────────────────────────────────────────────────────────────
//!
//! * 只挡"打到内网"，不挡"打公网的某个端口"。要更严就得限制端口白名单
//!   （80/443），但那会挡住合法的自建回调服务，暂不做。
//! * 真正的强隔离是把投递放进独立的 egress 代理 / 单独 worker，
//!   用网络策略而不是进程内判断来挡 —— 那是后续的事，当前这层是必要但不充分。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// 校验失败的原因。分成三类是因为调用方对它们的态度不同：
/// * 保存配置时：`Malformed` / `Blocked` 直接拒绝，`DnsFailed` 只告警
///   （DNS 抖动不该让商户存不了配置，投递时还会再校验一次）；
/// * 投递时：三类都是"放弃本次投递"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlReject {
    /// URL 本身不合法（协议不支持、没有主机、端口非法…）
    Malformed(String),
    /// 解析到了不允许的地址（内网 / 回环 / 元数据…）
    Blocked(String),
    /// 域名解析失败
    DnsFailed(String),
}

impl UrlReject {
    pub fn message(&self) -> String {
        match self {
            Self::Malformed(m) | Self::Blocked(m) | Self::DnsFailed(m) => m.clone(),
        }
    }
}

/// 校验通过的目标。
#[derive(Debug, Clone)]
pub struct OutboundTarget {
    pub url: reqwest::Url,
    /// `Some((域名, 地址))`：调用方应把它传给 `reqwest::ClientBuilder::resolve`，
    /// 让 reqwest 连**这个**地址而不是重新解析 DNS。IP 字面量目标为 `None`（无需解析）。
    pub pin: Option<(String, SocketAddr)>,
}

/// 判断一个 IP 是否**不允许**作为出站目标。
///
/// 返回 `Some(原因)` 表示拒绝。`None` 表示可以放行（公网地址）。
pub fn blocked_reason(ip: IpAddr) -> Option<String> {
    match ip {
        IpAddr::V4(v4) => blocked_reason_v4(v4),
        IpAddr::V6(v6) => blocked_reason_v6(v6),
    }
}

fn blocked_reason_v4(ip: Ipv4Addr) -> Option<String> {
    let o = ip.octets();
    let reason = if ip.is_unspecified() {
        "0.0.0.0（未指定地址）"
    } else if ip.is_loopback() {
        "回环地址（127.0.0.0/8）"
    } else if ip.is_private() {
        "内网地址（10/8、172.16/12、192.168/16）"
    } else if ip.is_link_local() {
        // 169.254.0.0/16，包含云厂商元数据 169.254.169.254
        "链路本地地址（169.254.0.0/16，含云厂商元数据 169.254.169.254）"
    } else if ip.is_broadcast() {
        "广播地址（255.255.255.255）"
    } else if ip.is_multicast() {
        "组播地址（224.0.0.0/4）"
    } else if o[0] == 100 && (64..=127).contains(&o[1]) {
        // 100.64.0.0/10：运营商级 NAT。云上常被用作内网网段。
        "运营商级 NAT 地址（100.64.0.0/10）"
    } else if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        // 192.0.0.0/24：IETF 协议专用（含 192.0.0.192 等特殊用途）
        "IETF 协议保留地址（192.0.0.0/24）"
    } else if o[0] == 192 && o[1] == 88 && o[2] == 99 {
        // 192.88.99.0/24：6to4 中继
        "6to4 中继地址（192.88.99.0/24）"
    } else if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
        // 198.18.0.0/15：基准测试保留
        "基准测试保留地址（198.18.0.0/15）"
    } else if (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
    {
        "文档示例地址（TEST-NET）"
    } else if o[0] >= 240 {
        // 240.0.0.0/4：保留（含 255.255.255.255，上面已拦）
        "保留地址（240.0.0.0/4）"
    } else {
        return None;
    };
    Some(reason.to_string())
}

fn blocked_reason_v6(ip: Ipv6Addr) -> Option<String> {
    if ip.is_unspecified() {
        return Some(":: （未指定地址）".to_string());
    }
    if ip.is_loopback() {
        return Some("回环地址（::1）".to_string());
    }
    // 组播
    if ip.is_multicast() {
        return Some("组播地址（ff00::/8）".to_string());
    }

    let seg = ip.segments();

    // `::ffff:a.b.c.d`（IPv4 映射）与 `::a.b.c.d`（IPv4 兼容）：
    // 内核会把它们路由到对应的 IPv4 地址，**必须**按 IPv4 规则判断，
    // 否则 `http://[::ffff:127.0.0.1]/` 就能绕过所有 v4 检查。
    if seg[0..5] == [0, 0, 0, 0, 0] && (seg[5] == 0xffff || seg[5] == 0) {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            (seg[6] & 0xff) as u8,
            (seg[7] >> 8) as u8,
            (seg[7] & 0xff) as u8,
        );
        // 只有 ::ffff:0:0/96 与 ::/96 两种形式才落到这里。
        // `::` 与 `::1` 已在上面拦掉，剩下的按 v4 规则判断。
        return blocked_reason_v4(v4)
            .map(|r| format!("IPv4 映射地址 {v4} → {r}"));
    }

    // 64:ff9b::/96（NAT64）：把 IPv4 嵌在低 32 位
    if seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0 {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            (seg[6] & 0xff) as u8,
            (seg[7] >> 8) as u8,
            (seg[7] & 0xff) as u8,
        );
        return blocked_reason_v4(v4).map(|r| format!("NAT64 地址 {v4} → {r}"));
    }

    // fc00::/7：唯一本地地址（IPv6 的内网）
    if (seg[0] & 0xfe00) == 0xfc00 {
        return Some("IPv6 唯一本地地址（fc00::/7）".to_string());
    }
    // fe80::/10：链路本地
    if (seg[0] & 0xffc0) == 0xfe80 {
        return Some("IPv6 链路本地地址（fe80::/10）".to_string());
    }
    // fec0::/10：站点本地（已废弃，但仍可能被路由）
    if (seg[0] & 0xffc0) == 0xfec0 {
        return Some("IPv6 站点本地地址（fec0::/10，已废弃）".to_string());
    }
    // 2001:db8::/32：文档示例
    if seg[0] == 0x2001 && seg[1] == 0x0db8 {
        return Some("IPv6 文档示例地址（2001:db8::/32）".to_string());
    }
    // 2001::/32：Teredo，内部封装 IPv4，无法可靠判断真身 → 拒绝
    if seg[0] == 0x2001 && seg[1] == 0 {
        return Some("IPv6 Teredo 隧道地址（2001::/32）".to_string());
    }
    None
}

/// 校验一个商户填写的回调地址。
///
/// 成功时返回可直接用于投递的目标（含应钉死的 IP）。
pub async fn check_outbound_url(raw: &str) -> Result<OutboundTarget, UrlReject> {
    let trimmed = raw.trim();
    let url = reqwest::Url::parse(trimmed).map_err(|e| UrlReject::Malformed(format!("URL 格式不正确: {e}")))?;

    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(UrlReject::Malformed(format!(
            "仅支持 http / https，收到 {scheme}"
        )));
    }

    // 先把 host 复制出来：下面要 move `url` 进返回值，而 `url.host()` 是借用。
    enum TargetHost {
        V4(Ipv4Addr),
        V6(Ipv6Addr),
        Domain(String),
    }

    let host = match url.host() {
        Some(url::Host::Ipv4(v4)) => TargetHost::V4(v4),
        Some(url::Host::Ipv6(v6)) => TargetHost::V6(v6),
        Some(url::Host::Domain(d)) => TargetHost::Domain(d.to_string()),
        None => return Err(UrlReject::Malformed("URL 缺少主机名".to_string())),
    };

    // 端口取显式值或协议默认值。`port()` 对非法端口（>65535）已在解析阶段失败。
    let port = url.port_or_known_default().unwrap_or(match scheme {
        "https" => 443,
        _ => 80,
    });

    match host {
        // ── IP 字面量：直接判断，不碰 DNS ──────────────────────────────
        TargetHost::V4(v4) => {
            if let Some(reason) = blocked_reason(IpAddr::V4(v4)) {
                return Err(UrlReject::Blocked(format!("目标地址不允许：{reason}")));
            }
            Ok(OutboundTarget {
                url,
                pin: None, // IP 字面量本来就不需要解析
            })
        }
        TargetHost::V6(v6) => {
            if let Some(reason) = blocked_reason(IpAddr::V6(v6)) {
                return Err(UrlReject::Blocked(format!("目标地址不允许：{reason}")));
            }
            Ok(OutboundTarget { url, pin: None })
        }
        // ── 域名：解析后逐个地址校验 ─────────────────────────────────
        TargetHost::Domain(domain) => {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((domain.as_str(), port))
                .await
                .map_err(|e| UrlReject::DnsFailed(format!("域名解析失败: {domain}: {e}")))?
                .collect();

            if addrs.is_empty() {
                return Err(UrlReject::DnsFailed(format!("域名 {domain} 没有解析到任何地址")));
            }

            // ⚠️ 只要有**任何一个**地址落在禁区就整体拒绝，而不是"挑一个合法的用"。
            // 攻击者可以控制 DNS 同时返回 `[公网IP, 127.0.0.1]`：如果这里挑公网 IP
            // 放行，而下游某处（或 rebinding）连到了内网那个，防线就没了。
            // 宁可误伤一个配置错误的域名，也不放过一个双面解析。
            for addr in &addrs {
                if let Some(reason) = blocked_reason(addr.ip()) {
                    return Err(UrlReject::Blocked(format!(
                        "域名 {domain} 解析到不允许的地址 {}：{reason}",
                        addr.ip()
                    )));
                }
            }

            let chosen = addrs[0];
            Ok(OutboundTarget {
                url,
                pin: Some((domain, chosen)),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(s: &str) -> bool {
        let ip: IpAddr = s.parse().expect("测试用 IP 必须合法");
        blocked_reason(ip).is_some()
    }

    #[test]
    fn blocks_private_and_loopback_v4() {
        for ip in [
            "127.0.0.1",
            "127.1.2.3",
            "10.0.0.1",
            "10.255.255.254",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.1",
            "0.0.0.0",
            "100.64.0.1",   // CGNAT
            "169.254.169.254", // 云元数据
            "192.0.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(blocked(ip), "{ip} 应被拒绝");
        }
    }

    #[test]
    fn allows_public_v4() {
        for ip in ["8.8.8.8", "1.1.1.1", "104.16.0.1", "172.32.0.1", "11.0.0.1"] {
            assert!(!blocked(ip), "{ip} 应放行");
        }
    }

    #[test]
    fn blocks_private_and_loopback_v6() {
        for ip in [
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "fec0::1",
            "2001:db8::1",
            "ff02::1",
        ] {
            assert!(blocked(ip), "{ip} 应被拒绝");
        }
    }

    /// 这条是绕过防线最常见的方式：IPv4 映射地址在内核里就是 IPv4。
    #[test]
    fn blocks_ipv4_mapped_v6() {
        assert!(blocked("::ffff:127.0.0.1"));
        assert!(blocked("::ffff:10.0.0.1"));
        assert!(blocked("::ffff:169.254.169.254"));
        // ::ffff:8.8.8.8 是公网，应当放行
        assert!(!blocked("::ffff:8.8.8.8"));
    }

    #[test]
    fn allows_public_v6() {
        assert!(!blocked("2606:4700:4700::1111"));
        assert!(!blocked("2400:3200::1"));
    }

    // ── 端到端：只测不需要 DNS 的 IP 字面量路径（离线可跑） ──────────────

    #[tokio::test]
    async fn rejects_metadata_ip_literal() {
        let err = check_outbound_url("http://169.254.169.254/latest/meta-data/")
            .await
            .expect_err("云元数据地址必须被拒绝");
        assert!(matches!(err, UrlReject::Blocked(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn rejects_localhost_ipv6_literal() {
        let err = check_outbound_url("http://[::1]:8080/hook")
            .await
            .expect_err("回环地址必须被拒绝");
        assert!(matches!(err, UrlReject::Blocked(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn accepts_public_ip_literal() {
        let target = check_outbound_url("https://1.1.1.1/hook")
            .await
            .expect("公网 IP 应通过");
        assert!(target.pin.is_none(), "IP 字面量不需要钉 DNS");
        assert_eq!(target.url.port_or_known_default(), Some(443));
    }

    #[tokio::test]
    async fn rejects_bad_scheme_and_missing_host() {
        for raw in ["ftp://example.com/hook", "file:///etc/passwd", "example.com/hook"] {
            let err = check_outbound_url(raw).await.expect_err("应被拒绝");
            assert!(matches!(err, UrlReject::Malformed(_)), "{raw} → {err:?}");
        }
    }
}
