# 客户 IP 全是 127.0.0.1（地图显示「暂无数据」）排障

> 症状：激活记录里的 `IP 地址` 一律是 `127.0.0.1`；商户后台「客户 IP 分布」地图显示「暂无客户 IP 分布数据」。
>
> 结论：**后端只看到了反向代理自己（127.0.0.1），没有采信 `X-Forwarded-For`。**
> 90% 的情况是 `.env` 里的 `TRUSTED_PROXIES` 没配、或配成了另一套部署形态的网段。

---

## 一、链路为什么断了

```
浏览器 ──公网──▶ Nginx(127.0.0.1:443) ──本机回环──▶ kamism-server(9527)
                        │                                │
                  追加 X-Forwarded-For              TCP 对端 = 127.0.0.1
                  X-Real-IP = 客户端 IP             ↑
                                            对端是否可信？看 TRUSTED_PROXIES
```

`utils/client_ip.rs` 的规则（有意设计成「默认不信任」）：

1. 只有**连接对端本身**落在 `TRUSTED_PROXIES` 里，才会去读 `X-Forwarded-For` / `X-Real-IP`；
2. 对端不可信 → 转发头**整条丢弃**，直接用对端地址。

所以只要 `.env` 里没有 `127.0.0.1`，后端拿到的一定是 `127.0.0.1`，与客户端真实 IP 无关。

### 地图为什么恰好是空，而不是显示「未知」

1. `activations.ip_address` 存的是字符串 `127.0.0.1`；
2. 仪表盘聚合时调 `ip_region::lookup()`，离线库把回环/内网段标成 `Reserved`，直接返回 `None`（`utils/ip_region.rs:127`）；
3. `ip_distribution` 于是是空数组 `[]`；
4. 前端 `ActivationMap.tsx` 判定 `domestic.length === 0 && abroad.length === 0`，渲染「暂无客户 IP 分布数据」。

所以地图空是**结果**，不是独立 bug。修复 IP 采集后，新产生的激活会自动出现在地图上。

---

## 二、远程已确认的现象（线上实测）

用商户账号登录 `https://km.fly-fly.fun/api` 拉取：

| 接口 | 结果 |
|---|---|
| `GET /merchant/dashboard-stats` | `ip_distribution: []`；`recent_activations` 5 条的 `ip_address` 全为 `127.0.0.1`，`ip_region` 全为 `null` |
| `GET /activations` | 共 10 条，10/10 为 `127.0.0.1` |
| `GET /auth/captcha` | `200` 正常返回算术题 → 线上后端已是含 `client_ip` 新逻辑的版本 |

**这说明不是代码问题，是配置问题。** 老版本取 XFF 最左段，反而不会落到 `127.0.0.1`；正是新逻辑的「对端不可信就忽略转发头」把配置缺失暴露成了 127.0.0.1。

---

## 三、三步确认（在服务器上执行）

```bash
# 1) 后端实际读到的配置与策略（重启后的日志里有一行）
sudo journalctl -u kamism -n 100 --no-pager | grep '客户端 IP 策略'
#   容器部署：docker logs kamism 2>&1 | grep '客户端 IP 策略' | tail -1
```

三种可能的输出与含义：

| 日志 | 含义 |
|---|---|
| `未配置 TRUSTED_PROXIES：忽略一切转发头...` | `.env` 里根本没这一项 → **就是它** |
| `已信任 1 条代理网段` 且 `TRUSTED_PROXIES="172.28.0.0/16"` | 配的是多容器的网段，但实际是裸机/单容器 → 对端 `127.0.0.1` 不在范围内 |
| `已信任 1 条代理网段` 且 `TRUSTED_PROXIES="127.0.0.1"` | 配置对了 → 去查第 2 步（Nginx 没传头） |

```bash
# 2) Nginx 到底有没有把客户端 IP 传下去
sudo nginx -T 2>/dev/null | grep -nE 'proxy_pass|proxy_set_header (X-Forwarded-For|X-Real-IP)'
```

必须能看到这两行同时指向 `/api/` 这个 location：

```nginx
proxy_set_header X-Real-IP       $remote_addr;
proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
```

缺任意一行，即使 `TRUSTED_PROXIES` 配对了也拿不到真实 IP。

```bash
# 3) 看库里到底存了什么
psql "$DATABASE_URL" -c "SELECT ip_address, count(*) FROM activations GROUP BY 1 ORDER BY 2 DESC LIMIT 20;"
```

---

## 四、修复

### 情况 A：裸机 / 单容器（Nginx 与后端同机、同网络命名空间）

```bash
# 有则改，无则加
grep -q '^TRUSTED_PROXIES=' /opt/kamism/.env \
  && sudo sed -i 's|^TRUSTED_PROXIES=.*|TRUSTED_PROXIES=127.0.0.1|' /opt/kamism/.env \
  || echo 'TRUSTED_PROXIES=127.0.0.1' | sudo tee -a /opt/kamism/.env

sudo systemctl restart kamism
sudo journalctl -u kamism -n 30 --no-pager | grep '客户端 IP 策略'
# 期望：已信任 1 条代理网段（TRUSTED_PROXIES="127.0.0.1"）
```

单容器（`docker-compose.standalone.yml`）同理，改宿主机 `.env` 后 `docker compose -f docker-compose.standalone.yml up -d`。

### 情况 B：多容器（Nginx 独立容器 → app 容器）

此时对端是 `172.28.0.x`（Nginx 容器），不是 `127.0.0.1`：

```bash
TRUSTED_PROXIES=172.28.0.0/16
```

这个网段必须与 `docker-compose.yml` 末尾 `networks.default.ipam.config.subnet` 一致。改 subnet 时两处一起改。

> 不要图省事写 `0.0.0.0/0` 或 `172.16.0.0/12`：前者等于信任任何来源，`X-Forwarded-For` 可以被任意伪造，IP 黑名单形同虚设；后者覆盖 172.16–172.31，比实际需要宽 16 倍。

---

## 五、不产生业务数据的验证方法

`TRUSTED_PROXIES` 只在启动时读取，必须重启。重启后**不用激活卡密**就能验证：

```bash
# 浏览器或本机打一次（任意一个带 IP 限流的接口）
curl -s -o /dev/null https://yourdomain.com/api/auth/captcha

# 看限流 key 的 IP 部分
redis-cli --scan --pattern 'rl:*'
# 期望：rl:captcha:203.0.113.45      ← 真实公网 IP
# 错误：rl:captcha:127.0.0.1        ← 还在退化
```

限流 key 全部由 `bucket_key(resolve(...))` 生成，所以这就是 `resolve()` 的返回值本身，比查库更直接。

最终验收仍建议真激活一张卡：`GET /activations` 里该条记录的 `ip_address` 应为公网 IP，`ip_region` 不再是 `null`，地图出现对应省份。

---

## 六、历史数据

已经落库的 `127.0.0.1` **无法还原**——真实 IP 当时就没采到，没有其他副本。

```sql
-- 先看量
SELECT count(*) FROM activations WHERE ip_address IN ('127.0.0.1', '::1');

-- 可选：把这些无效值置空，避免「激活记录」列表继续展示误导性的 127.0.0.1
--       地图本来就不统计它们（Reserved → None），置空不改变地图结果
UPDATE activations SET ip_address = NULL WHERE ip_address IN ('127.0.0.1', '::1');
```

不建议直接删激活记录：那是计费/审计凭据，`ip_address` 只是附带信息。

---

## 七、这个 bug 的连带影响（比地图更严重）

在 IP 退化成 `127.0.0.1` 的这段时间里：

| 能力 | 实际状态 |
|---|---|
| 公开 API 限流 | `rl:api:127.0.0.1` — **全站所有用户共用一个桶，60 次/分钟**。一个客户端打满，全站 429 |
| 登录限流 | `rl:login:127.0.0.1` — 全站共享 10 次/分钟，攻击者可以帮你把后台登录锁死 |
| 激活限流 | `rl:activate:127.0.0.1` — 同 IP 高频激活告警恒成立，等于没有区分度 |
| IP 黑名单 | 商户拉黑任意 IP 都不生效；拉黑 `127.0.0.1` 会误封全站 |
| 审计日志 / 风控告警 | IP 字段无意义，事后无法追溯 |

所以这不是「地图不好看」的展示问题，**限流与风控实际上处于失效状态**，优先级应当和 P0 同级。

---

## 八、防复发

- 启动日志已打印 `客户端 IP 策略`，发版验收时固定检查这一行（README「验收」一节已写入）。
- 部署到新环境时，`.env` 从对应形态的模板复制：裸机/单容器用 `127.0.0.1`，多容器用 compose 的 subnet，**不要混用**。
- 若前面还有 CDN（Cloudflare 等），Nginx 的 `$remote_addr` 会是 CDN 回源 IP，此时需把 CDN 回源网段加入 `TRUSTED_PROXIES`，或让 Nginx 用 `CF-Connecting-IP` 重写 `X-Real-IP`。当前线上响应头没有 CDN 特征，暂不涉及。
