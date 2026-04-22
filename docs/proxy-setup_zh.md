# 住宅代理配置指南 (IPRoyal)

Claude Ultra 支持住宅代理轮换，避免基于 IP 的速率限制。本指南介绍如何配置 IPRoyal 住宅代理。

## 第 1 步：注册 IPRoyal 账号

1. 打开 [https://iproyal.com](https://iproyal.com/?r=claude-ultra)
2. 注册账号
3. 在控制台中进入 **Residential Proxies**（住宅代理）

## 第 2 步：购买住宅代理流量

1. 在 IPRoyal 控制台，前往 **Residential Proxies → Purchase**
2. 选择流量套餐（按 GB 计费）
   - 建议首次购买 **1 GB**（约 $7）用于测试
   - 流量不过期
3. 完成付款

## 第 3 步：获取凭证

1. 在 IPRoyal 控制台，前往 **Residential Proxies → Setup**
2. 你会看到：
   - **Username**（用户名，如 `customer-xxxx`）
   - **Password**（密码，一串长字符）
3. 记下这些凭证，稍后填入 Claude Ultra

## 第 4 步：在 Claude Ultra 中配置

1. 打开 Claude Ultra → **设置 → 代理**
2. 启用 **住宅代理**
3. 填入：

| 字段 | 值 |
|------|-----|
| Host | `geo.iproyal.com`（默认值，无需修改） |
| Port | `12321`（默认值，无需修改） |
| Username | 你的 IPRoyal 用户名（如 `customer-xxxx`） |
| Password | 你的 IPRoyal 密码 |
| Country | `us`（默认）或任意[支持的国家代码](https://iproyal.com/locations/) |

4. 点击 **保存**

或直接编辑 `~/.claude-ultra/config.json`：

```json
{
  "proxy": {
    "default_type": "residential",
    "residential": {
      "host": "geo.iproyal.com",
      "port": 12321,
      "username": "customer-xxxx",
      "password": "your_password_here",
      "default_country": "us"
    }
  }
}
```

## 第 5 步：验证

保存后，Claude Ultra 会通过 IPRoyal 住宅代理路由 API 请求。每个账号会话分配一个固定 IP（保持 24 小时），会话之间 IP 自动轮换。

可在 **流量日志** 中查看代理使用情况——代理列会显示分配的 IP。

## 工作原理

- 每个 Claude 账号分配一个 **固定住宅 IP**，持续 24 小时
- 账号轮换时生成新的 session ID → 新 IP
- 流量按 GB 计费，由 IPRoyal 收取（不是 Claude Ultra）
- 如果代理凭证为空，Claude Ultra 直连（不使用代理）

## 支持的国家

IPRoyal 支持 195+ 个国家。常用选择：

| 国家 | 代码 |
|------|------|
| 美国 | `us` |
| 英国 | `gb` |
| 德国 | `de` |
| 日本 | `jp` |
| 新加坡 | `sg` |

完整列表：[https://iproyal.com/locations/](https://iproyal.com/locations/)

## 常见问题

**"代理认证失败"**
- 检查 设置 → 代理 中的用户名和密码
- 确保购买的是 **Residential Proxies**（住宅代理），不是 Datacenter 或 ISP

**"通过代理连接超时"**
- 检查 IPRoyal 余额（0 GB = 拒绝连接）
- 尝试切换国家代码

**不用代理正常，用代理失败**
- 某些地区住宅 IP 较慢，尝试 `us` 或 `gb`
- 查看 IPRoyal 控制台的服务状态
