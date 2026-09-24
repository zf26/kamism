//! 离线 IP 归属地解析（ip2region v4 xdb 格式）。
//!
//! 为什么不用在线 API / 为什么自己实现而不引入 `ip2region` crate：
//!   - 商户的客户 IP 是隐私数据，交给第三方在线 API 会外泄，且免费版有频率限制；
//!   - `ip2region` crate（openmynet v0.1.0）依赖 `speedy`，且 `Searcher::new` 只收文件路径，
//!     无法从 `include_bytes!` 内嵌的字节直接构造（字段私有）；
//!   - xdb 的查询算法很短（头部 256 字节 + 向量索引 + 二分查找），照抄官方 binding
//!     （`lionsoul2014/ip2region` 的 rust 实现）即可，零额外依赖、零运行时文件路径问题。
//!
//! 数据文件 `assets/ip2region_v4.xdb` 通过 `include_bytes!` 编译进二进制（约 10.6MB），
//! 因此部署时不需要额外挂载数据文件，升级数据只需替换文件后重新编译。
//!
//! 关于「查不到」的处理（仓库纪律：不允许安静地做错事）：
//!   `lookup` 返回 `Option<Region>`，调用方对 `None` 的处理（降级显示「未知」还是告警）
//!   由调用方决定。这里不吞错、不 panic——查不到就是 `None`，内网 IP / 未收录 IP /
//!   非法 IP 都走 `None`，但会带一条 `trace!`（避免刷屏）留痕。

use std::net::Ipv4Addr;

/// xdb 头部固定 256 字节（version + index_policy + created_at + start_index_ptr + end_index_ptr）
const HEADER_INFO_LENGTH: u32 = 256;
/// 向量索引：256 行 × 256 列
const VECTOR_INDEX_COLS: u32 = 256;
/// 每个向量索引条目 8 字节（s_ptr + e_ptr）
const VECTOR_INDEX_SIZE: u32 = 8;
/// 每个二分索引段 14 字节（start_ip 4 + end_ip 4 + data_len 2 + data_index 4）
const SEGMENT_INDEX_SIZE: u32 = 14;

/// 编译期内嵌 xdb 数据。文件缺失会导致编译失败——这正是我们想要的：
/// 数据是功能的一部分，不能静默缺位。
static XDB: &[u8] = include_bytes!("../../assets/ip2region_v4.xdb");

/// 结构化归属地。字段与 xdb 的 `国家|区域|省|市|ISP` 管道格式一一对应。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Region {
    pub country: Option<String>,
    pub province: Option<String>,
    pub city: Option<String>,
    pub isp: Option<String>,
}

impl Region {
    /// 给前端/日志用的精简展示串，如「广东深圳」「广东·深圳」。
    /// 优先级：省市都在 → 「省+市」；只有省 → 省；只有市 → 市；
    /// 都没有但国家在（国外 IP）→ 国家。都没有 → None。
    pub fn display(&self) -> Option<String> {
        match (&self.province, &self.city) {
            (Some(p), Some(c)) => {
                // 直辖市（北京/上海/天津/重庆）省市同名，避免显示「北京北京」
                if p == c {
                    Some(p.clone())
                } else {
                    Some(format!("{}{}", p, c))
                }
            }
            (Some(p), None) => Some(p.clone()),
            (None, Some(c)) => Some(c.clone()),
            (None, None) => self.country.clone(),
        }
    }
}

/// 解析单个 IPv4 的归属地。查不到（内网 IP / 未收录 / 非法输入）返回 `None`。
///
/// 算法照抄 ip2region 官方 rust binding 的 `Searcher::search`：
/// 用 IP 前两字节定位向量索引，拿到该段的二分查找区间，再在区间内二分命中。
pub fn lookup(ip: &str) -> Option<Region> {
    let addr: Ipv4Addr = ip.trim().parse().ok()?;
    let ip_u32 = u32::from_be_bytes(addr.octets());

    let il0 = (ip_u32 >> 24) & 0xFF;
    let il1 = (ip_u32 >> 16) & 0xFF;

    // 向量索引条目的字节偏移
    let idx = il0 * VECTOR_INDEX_COLS * VECTOR_INDEX_SIZE + il1 * VECTOR_INDEX_SIZE;
    let offset = HEADER_INFO_LENGTH + idx;

    if offset as usize + 8 > XDB.len() {
        tracing::trace!("IP 归属地：向量索引越界 ip={}", ip);
        return None;
    }

    let s_ptr = u32::from_le_bytes(XDB[offset as usize..offset as usize + 4].try_into().ok()?);
    let e_ptr = u32::from_le_bytes(XDB[offset as usize + 4..offset as usize + 8].try_into().ok()?);

    // 该 IP 段在索引里没有数据（官方语义：返回空）
    if s_ptr == 0 || e_ptr == 0 || e_ptr <= s_ptr {
        return None;
    }

    let mut l: u32 = 0;
    let mut h: u32 = (e_ptr - s_ptr) / SEGMENT_INDEX_SIZE;

    while l < h {
        let m = (l + h) >> 1;
        let p = (s_ptr + m * SEGMENT_INDEX_SIZE) as usize;

        // 段边界检查：越界说明 xdb 数据损坏或指针错误，不能 panic
        if p + SEGMENT_INDEX_SIZE as usize > XDB.len() {
            tracing::warn!("IP 归属地：二分索引越界 ip={} ptr={}", ip, p);
            return None;
        }

        let start_ip = u32::from_le_bytes(XDB[p..p + 4].try_into().ok()?);
        let end_ip = u32::from_le_bytes(XDB[p + 4..p + 8].try_into().ok()?);

        if ip_u32 < start_ip {
            h = m;
        } else if ip_u32 > end_ip {
            l = m + 1;
        } else {
            // 命中：读 data_len(2) + data_index(4)，从 data_index 起取 data_len 字节
            let data_len = u16::from_le_bytes(XDB[p + 8..p + 10].try_into().ok()?) as usize;
            let data_ptr = u32::from_le_bytes(XDB[p + 10..p + 14].try_into().ok()?) as usize;

            let end = data_ptr + data_len;
            if end > XDB.len() {
                tracing::warn!("IP 归属地：数据区越界 ip={} data_ptr={} data_len={}", ip, data_ptr, data_len);
                return None;
            }

            let raw = std::str::from_utf8(&XDB[data_ptr..end]).ok()?;
            let region = parse_region(raw);
            // 保留地址（127.0.0.1、192.168.x.x 等内网/回环/组播段）在 xdb 里标记为
            // `Reserved`，对商户没有任何「归属地」意义，和查不到一样返回 None，
            // 避免界面上出现「Reserved」这种没意义的地名。
            if region.country.as_deref() == Some("Reserved")
                || region.province.as_deref() == Some("Reserved")
            {
                return None;
            }
            return Some(region);
        }
    }

    None
}

/// 把 xdb 的管道串解析成结构化 Region。
///
/// ⚠️ 字段顺序取决于 xdb 版本：
///   - v2（`ip2region.xdb`）：`国家|区域|省|市|ISP`（5 段，第 2 段是「区域」）
///   - v4（`ip2region_v4.xdb`，本项目用这个）：`国家|省|市|ISP|国家代码`（5 段，无「区域」段）
/// 本项目内嵌的是 v4 数据（header version=3），所以用 v4 的字段布局。
/// 实测：`120.24.78.129` → `中国|广东省|深圳市|阿里|CN`。
/// `0` 或空串表示该字段缺省（ip2region 的约定）。
fn parse_region(raw: &str) -> Region {
    let parts: Vec<&str> = raw.split('|').collect();
    let pick = |i: usize| -> Option<String> {
        parts.get(i).and_then(|s| {
            let t = s.trim();
            if t.is_empty() || t == "0" {
                None
            } else {
                Some(t.to_string())
            }
        })
    };

    Region {
        country: pick(0),
        province: pick(1),
        city: pick(2),
        isp: pick(3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_region_handles_full_and_partial() {
        // v4 格式：`国家|省|市|ISP|国家代码`
        let r = parse_region("中国|广东省|深圳市|阿里|CN");
        assert_eq!(r.country.as_deref(), Some("中国"));
        assert_eq!(r.province.as_deref(), Some("广东省"));
        assert_eq!(r.city.as_deref(), Some("深圳市"));
        assert_eq!(r.isp.as_deref(), Some("阿里"));

        // 0 和空串都是「缺省」
        let r = parse_region("中国|江苏省|南京市|0|CN");
        assert_eq!(r.province.as_deref(), Some("江苏省"));
        assert_eq!(r.city.as_deref(), Some("南京市"));
        assert_eq!(r.isp, None);

        // 国外 IP：只有国家
        let r = parse_region("United States|California|0|Google LLC|US");
        assert_eq!(r.country.as_deref(), Some("United States"));
        assert_eq!(r.province.as_deref(), Some("California"));
        assert_eq!(r.city, None);
    }

    #[test]
    fn display_merges_province_city_and_handles_direct_city() {
        assert_eq!(
            Region { province: Some("广东省".into()), city: Some("深圳市".into()), ..Default::default() }
                .display().as_deref(),
            Some("广东省深圳市")
        );
        // 直辖市省市同名 → 只显示一次
        assert_eq!(
            Region { province: Some("北京市".into()), city: Some("北京市".into()), ..Default::default() }
                .display().as_deref(),
            Some("北京市")
        );
        // 只有省
        assert_eq!(
            Region { province: Some("广东省".into()), city: None, ..Default::default() }
                .display().as_deref(),
            Some("广东省")
        );
        // 都没有
        assert_eq!(Region::default().display(), None);
    }

    #[test]
    fn lookup_rejects_invalid_and_private_ip() {
        // 非法输入不 panic，返回 None
        assert!(lookup("not-an-ip").is_none());
        assert!(lookup("").is_none());
        assert!(lookup("999.1.1.1").is_none());
    }

    #[test]
    fn lookup_treats_reserved_addresses_as_none() {
        // 保留地址（回环/内网/组播）在 xdb 里是 `Reserved`，必须降级为 None，
        // 不能把「Reserved」当归属地展示给商户。
        assert!(lookup("127.0.0.1").is_none(), "回环地址应返回 None");
        assert!(lookup("192.168.1.1").is_none(), "内网地址应返回 None");
        assert!(lookup("10.0.0.1").is_none(), "内网地址应返回 None");
    }

    #[test]
    fn lookup_known_public_ip_returns_region() {
        // 阿里云深圳的公开 IP（官方 binding 的测试用例用 120.24.78.129）
        // 结果依赖 xdb 数据版本，这里只断言「能查到且不 panic」，不断言具体城市，
        // 因为数据更新会改归属。
        let r = lookup("120.24.78.129");
        assert!(r.is_some());
        let r = r.unwrap();
        // 至少能给出一个可展示的地名
        assert!(r.province.is_some() || r.city.is_some() || r.country.is_some());
    }
}
