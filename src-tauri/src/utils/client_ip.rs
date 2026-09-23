//! 客户端真实 IP 解析（可信代理链）
//!
//! 为什么需要这个模块：服务一般部署在 Nginx 之后，而 Nginx 用
//! `$proxy_add_x_forwarded_for` 转发，语义是「在客户端自己发来的 X-Forwarded-For
//! 基础上追加 Nginx 看到的对端地址」，于是到达后端的头部形如：
//!
//! ```text
//! X-Forwarded-For: <客户端可以随便编的值>, <Nginx 亲眼看到的客户端 IP>
//! ```
//!
//! 旧实现取的是「第一段」，正好是客户端可以伪造的那一段 —— 于是 IP 黑名单能被
//! 一个请求头绕过，`activations.ip_address`、异常告警里的 IP 全都不可信。
//!
//! 这里改成两条规则：
//! 1. 只有**连接对端本身**是可信代理时，才去读转发头；否则一律用对端地址，
//!    头部完全忽略（这是防伪造的关键：不可信来源说什么都不算）。
//! 2. 读转发头时**从右往左**找第一个非可信代理的地址 —— 越靠右越是代理亲眼
//!    所见，越靠左越可能是客户端编的。
//!
//! 信任范围由 `TRUSTED_PROXIES` 配置（逗号分隔的 IP 或 CIDR），默认**不信任任何
//! 代理**。默认不信任是刻意的：宁可让 IP 维度暂时退化成「代理的 IP」（可观测、
//! 可在日志里发现），也不要默认相信一个可以被伪造的头。

use axum::http::HeaderMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

/// 只回看转发头最右侧的 N 条，防止超长头部放大解析开销
const MAX_FORWARDED_ENTRIES: usize = 20;

/// 一条可信网络：`10.0.0.0/8`、`127.0.0.1`（等价 `/32`）、`::1`（等价 `/128`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpNet {
    V4 { base: u32, prefix: u8 },
    V6 { base: u128, prefix: u8 },
}

impl IpNet {
    fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            return None;
        }
        let (addr_part, prefix_part) = match spec.split_once('/') {
            Some((a, p)) => (a.trim(), Some(p.trim())),
            None => (spec, None),
        };
        let ip: IpAddr = addr_part.parse().ok()?;
        let prefix = match prefix_part {
            Some(p) => p.parse::<u8>().ok()?,
            None => match ip {
                IpAddr::V4(_) => 32,
                IpAddr::V6(_) => 128,
            },
        };
        match ip {
            IpAddr::V4(v4) => {
                if prefix > 32 {
                    return None;
                }
                Some(IpNet::V4 { base: u32::from(v4), prefix })
            }
            IpAddr::V6(v6) => {
                if prefix > 128 {
                    return None;
                }
                Some(IpNet::V6 { base: u128::from(v6), prefix })
            }
        }
    }

    fn contains(&self, ip: IpAddr) -> bool {
        let ip = normalize(ip);
        match (self, ip) {
            (IpNet::V4 { base, prefix }, IpAddr::V4(v4)) => {
                let mask = v4_mask(*prefix);
                (u32::from(v4) & mask) == (*base & mask)
            }
            (IpNet::V6 { base, prefix }, IpAddr::V6(v6)) => {
                let mask = v6_mask(*prefix);
                (u128::from(v6) & mask) == (*base & mask)
            }
            // 跨协议族不匹配：/8 的 IPv4 网段不会包含一个纯 IPv6 地址
            _ => false,
        }
    }
}

fn v4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn v6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

/// `::ffff:1.2.3.4` 与 `1.2.3.4` 视为同一个地址，否则双栈监听下会匹配不上可信网段
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// 启动时解析一次、之后只读的可信代理策略
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    networks: Vec<IpNet>,
}

impl TrustedProxies {
    /// 从环境变量 `TRUSTED_PROXIES` 读取，空或未设置 = 不信任任何代理
    pub fn from_env() -> Self {
        Self::parse(&std::env::var("TRUSTED_PROXIES").unwrap_or_default())
    }

    pub fn parse(spec: &str) -> Self {
        let networks = spec
            .split(',')
            .filter_map(IpNet::parse)
            .collect::<Vec<_>>();
        TrustedProxies { networks }
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.networks.iter().any(|n| n.contains(ip))
    }

    /// 启动日志用：把生效的策略说清楚，避免「以为配了其实没配」
    pub fn describe(&self) -> String {
        if self.networks.is_empty() {
            "未配置 TRUSTED_PROXIES：忽略一切转发头，客户端 IP 取连接对端地址".to_string()
        } else {
            format!("已信任 {} 条代理网段（TRUSTED_PROXIES）", self.networks.len())
        }
    }

    /// 解析真实客户端 IP。对端不可信时头部一律忽略。
    pub fn resolve(&self, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
        let peer_ip = normalize(peer.ip());

        // 规则 1：对端不是可信代理 → 不看任何转发头
        if !self.contains(peer_ip) {
            return peer_ip;
        }

        // 规则 2：从右往左取第一个非可信地址（代理亲眼看到的客户端）
        if let Some(raw) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            let entries: Vec<&str> = raw
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            let take = entries.len().min(MAX_FORWARDED_ENTRIES);
            let tail = &entries[entries.len() - take..];

            let mut leftmost_parsed: Option<IpAddr> = None;
            for entry in tail.iter().rev() {
                if let Ok(ip) = entry.parse::<IpAddr>() {
                    let ip = normalize(ip);
                    leftmost_parsed = Some(ip);
                    if !self.contains(ip) {
                        return ip;
                    }
                }
            }
            // 整条链都是可信代理：说明客户端 IP 没被记录，退回最左侧一项，
            // 至少不会误把代理自己的 IP 当成客户端
            if let Some(ip) = leftmost_parsed {
                return ip;
            }
        }

        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
        {
            return normalize(ip);
        }

        peer_ip
    }
}

/// 限流桶的聚合粒度
///
/// - IPv4：用完整地址，一个地址一个桶
/// - IPv6：收敛到 /64 前缀
///
/// 为什么 IPv6 必须收敛：家用/机房 IPv6 通常分配一个 /64 前缀，也就是同一个用户
/// 手里握有 2^64 个可用地址 —— 按完整地址限流等于没限流（每个请求换一个末段即可绕过），
/// 「防黄牛」会彻底失效。按 /64 聚合才是「一个用户一个桶」。
pub fn bucket_key(ip: IpAddr) -> String {
    match normalize(ip) {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let masked = u128::from(v6) & v6_mask(64);
            format!("{}/64", Ipv6Addr::from(masked))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn headers_with(name: &str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    fn peer(s: &str) -> SocketAddr {
        SocketAddr::new(s.parse::<IpAddr>().unwrap(), 12345)
    }

    #[test]
    fn untrusted_peer_ignores_forwarded_headers() {
        // 这是防伪造的核心断言：直连或不可信反代时，伪造的 XFF 必须无效
        let tp = TrustedProxies::default();
        let h = headers_with("x-forwarded-for", "1.2.3.4");
        assert_eq!(
            tp.resolve(&h, peer("9.9.9.9")),
            "9.9.9.9".parse::<IpAddr>().unwrap()
        );

        // 同理，X-Real-IP 也不能被不可信来源左右
        let h = headers_with("x-real-ip", "1.2.3.4");
        assert_eq!(
            tp.resolve(&h, peer("9.9.9.9")),
            "9.9.9.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn trusted_peer_takes_rightmost_untrusted() {
        // Nginx 用 $proxy_add_x_forwarded_for：左侧是客户端伪造的，右侧是真实客户端。
        //
        // 头值必须是 ASCII：HeaderValue::to_str() 拒绝非 ASCII，会让整条 XFF
        // 被判为不可用而整体跳过（于是 resolve 退回对端地址）。这不是本模块的
        // 逻辑问题，但用中文写头值会导致测试测不到真正想测的解析逻辑。
        let tp = TrustedProxies::parse("127.0.0.1");
        let h = headers_with("x-forwarded-for", "fake-left, 5.6.7.8");
        assert_eq!(
            tp.resolve(&h, peer("127.0.0.1")),
            "5.6.7.8".parse::<IpAddr>().unwrap()
        );

        // 客户端伪造多段也不影响：只有最右侧那一段是代理亲见的
        let h = headers_with("x-forwarded-for", "1.1.1.1, 2.2.2.2, 5.6.7.8");
        assert_eq!(
            tp.resolve(&h, peer("127.0.0.1")),
            "5.6.7.8".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn two_hop_chain_skips_trusted_hops() {
        // 边缘 Nginx(10.0.0.2) → 内层 Nginx(127.0.0.1) → 后端
        // 到后端时 XFF = "客户端编的, 真实客户端, 10.0.0.2"
        let tp = TrustedProxies::parse("127.0.0.1,10.0.0.2");
        let h = headers_with("x-forwarded-for", "6.6.6.6, 5.6.7.8, 10.0.0.2");
        assert_eq!(
            tp.resolve(&h, peer("127.0.0.1")),
            "5.6.7.8".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn cidr_and_single_ip_matching() {
        // 这一组不要混入 0.0.0.0/0 —— 它覆盖一切，「不在集合内」的断言必然失败。
        // （这条测试最初就是把两者写在一组里，导致自相矛盾。）
        let tp = TrustedProxies::parse("10.0.0.0/8, 192.168.1.5");
        assert!(tp.contains("10.1.2.3".parse().unwrap()));
        assert!(tp.contains("192.168.1.5".parse().unwrap()));
        // 单个 IP 等价于 /32，不该放宽成整个网段
        assert!(!tp.contains("192.168.1.6".parse().unwrap()));
        assert!(!tp.contains("8.8.8.8".parse().unwrap()));

        // 0.0.0.0/0 单独验证：确实覆盖全部 IPv4
        let all = TrustedProxies::parse("0.0.0.0/0");
        assert!(all.contains("8.8.8.8".parse().unwrap()));
        assert!(all.contains("192.168.1.6".parse().unwrap()));

        // /32 显式写法同样不放宽
        let tp = TrustedProxies::parse("192.168.1.5/32");
        assert!(tp.contains("192.168.1.5".parse().unwrap()));
        assert!(!tp.contains("192.168.1.6".parse().unwrap()));
    }

    #[test]
    fn malformed_specs_are_ignored_not_panicking() {
        let tp = TrustedProxies::parse("10.0.0.0/33, 不是IP, , 300.1.1.1, ::1/128");
        assert!(tp.contains("::1".parse().unwrap()));
        assert!(!tp.contains("10.1.1.1".parse().unwrap()));
        assert!(!tp.is_empty());

        let tp = TrustedProxies::parse("全部非法");
        assert!(tp.is_empty());
    }

    #[test]
    fn all_trusted_chain_falls_back_to_leftmost() {
        let tp = TrustedProxies::parse("127.0.0.1");
        let h = headers_with("x-forwarded-for", "127.0.0.1, 127.0.0.1");
        assert_eq!(
            tp.resolve(&h, peer("127.0.0.1")),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn ipv4_mapped_ipv6_matches_trusted_v4() {
        let tp = TrustedProxies::parse("127.0.0.1");
        let h = headers_with("x-forwarded-for", "8.8.4.4");
        assert_eq!(
            tp.resolve(&h, peer("::ffff:127.0.0.1")),
            "8.8.4.4".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn garbage_entries_do_not_break_resolution() {
        let tp = TrustedProxies::parse("127.0.0.1");
        let h = headers_with("x-forwarded-for", "unknown, 5.6.7.8");
        assert_eq!(
            tp.resolve(&h, peer("127.0.0.1")),
            "5.6.7.8".parse::<IpAddr>().unwrap()
        );

        let h = headers_with("x-forwarded-for", "unknown, garbage");
        assert_eq!(
            tp.resolve(&h, peer("127.0.0.1")),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn x_real_ip_used_when_forwarded_for_absent() {
        let tp = TrustedProxies::parse("127.0.0.1");
        let h = headers_with("x-real-ip", "5.6.7.8");
        assert_eq!(
            tp.resolve(&h, peer("127.0.0.1")),
            "5.6.7.8".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn v6_peer_and_network() {
        let tp = TrustedProxies::parse("::1");
        let h = headers_with("x-forwarded-for", "2001:db8::1");
        assert_eq!(
            tp.resolve(&h, peer("::1")),
            "2001:db8::1".parse::<IpAddr>().unwrap()
        );
        // 纯 IPv6 客户端不会被 IPv4 网段误判
        assert!(!TrustedProxies::parse("10.0.0.0/8").contains(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn localhost_v4_constant_is_not_special_cased() {
        // 只有显式配置才信任本机，默认不信任（避免「以为本机就是可信」的错觉）
        assert!(!TrustedProxies::default().contains(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }

    #[test]
    fn bucket_key_keeps_ipv4_as_is() {
        assert_eq!(bucket_key("1.2.3.4".parse().unwrap()), "1.2.3.4");
        // IPv4-mapped 形式要归一化，避免同一个客户端因双栈产生两个桶
        assert_eq!(bucket_key("::ffff:1.2.3.4".parse().unwrap()), "1.2.3.4");
    }

    #[test]
    fn bucket_key_collapses_ipv6_to_64_prefix() {
        let a = bucket_key("2001:db8:1:2::5".parse().unwrap());
        let b = bucket_key("2001:db8:1:2:ffff:ffff:ffff:ffff".parse().unwrap());
        let c = bucket_key("2001:db8:1:3::5".parse().unwrap());

        // 同一个 /64 内的不同地址必须落到同一个桶（否则换末段就能绕过限流）
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, "2001:db8:1:2::/64");
    }
}
